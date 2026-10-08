# Rust Cross-Compilation for Yocto/ARM64 Targets

## Purpose

This document describes how to cross-compile a Rust application on an x86_64 Ubuntu development machine for an ARM64 Linux/Yocto target using the **target device's root filesystem as the sysroot**.

The procedure was successfully validated for:

* Target: Renesas R-Car V4H WhiteHawk
* Architecture: AArch64
* OS: Yocto / Poky
* Yocto: Dunfell 3.1.11
* Target glibc: 2.31
* Target GCC: 9.3.0
* Rust: 1.98.1
* Cargo: 1.98.1
* Host: x86_64 Ubuntu
* Host cross compiler: `/usr/bin/aarch64-linux-gnu-gcc` GCC 11.x
* Target rootfs/sysroot:

```text
/nfs_share/v4h_whitehawk
```

The resulting Rust binary successfully executed on WhiteHawk.

---

# 1. Core Principle

For cross-compilation, three things matter:

```text
Rust target
    +
host-side cross compiler
    +
target device rootfs/sysroot
```

For WhiteHawk:

```text
Rust
  |
  | cargo build --release --target aarch64-unknown-linux-gnu
  v
/usr/bin/aarch64-linux-gnu-gcc
  |
  | --sysroot=/nfs_share/v4h_whitehawk
  |
  +-- target startup objects
  +-- target libc
  +-- target dynamic linker
  +-- target libgcc_s
  +-- target libraries
  |
  v
DeviceAgent
  |
  v
WhiteHawk ARM64
```

The most important rule is:

> **The final binary must be linked against libraries from the target device, not against the development machine's newer Ubuntu libraries.**

This is particularly important when the development machine has a newer glibc than the target.

---

# 2. WhiteHawk Target Information

WhiteHawk reported:

```text
Architecture:       aarch64
Kernel:             5.10.147-yocto-standard
OS:                 Poky (Yocto Project Reference Distro)
Yocto:              3.1.11 (Dunfell)
GCC:                9.3.0
Make:               4.3
glibc:              2.31
```

Target rootfs:

```text
/nfs_share/v4h_whitehawk
```

Important target libraries:

```text
/nfs_share/v4h_whitehawk/lib/libc.so.6
/nfs_share/v4h_whitehawk/lib/libm.so.6
/nfs_share/v4h_whitehawk/lib/libpthread.so.0
/nfs_share/v4h_whitehawk/lib/libdl.so.2
/nfs_share/v4h_whitehawk/lib/librt.so.1
/nfs_share/v4h_whitehawk/lib/libgcc_s.so.1
```

Startup objects:

```text
/nfs_share/v4h_whitehawk/usr/lib/crti.o
/nfs_share/v4h_whitehawk/usr/lib/crt1.o
/nfs_share/v4h_whitehawk/usr/lib/crtn.o
/nfs_share/v4h_whitehawk/usr/lib/Scrt1.o
```

Target dynamic loader:

```text
/nfs_share/v4h_whitehawk/lib/ld-linux-aarch64.so.1
```

---

# 3. Verify the Target Rootfs

Before building for any device, inspect its rootfs.

## Check architecture

```bash
file /nfs_share/v4h_whitehawk/lib/libc.so.6
```

Expected:

```text
ELF 64-bit LSB shared object, ARM aarch64, ...
```

## Check glibc versions

```bash
strings /nfs_share/v4h_whitehawk/lib/libc.so.6 |
    grep -o 'GLIBC_[0-9.]*' |
    sort -Vu
```

For WhiteHawk, the available versions included approximately:

```text
GLIBC_2.17
...
GLIBC_2.30
GLIBC_PRIVATE
```

Therefore the generated binary must not require:

```text
GLIBC_2.32
GLIBC_2.33
GLIBC_2.34
```

or anything newer than what the target provides.

## Check dynamic linker

```bash
ls -l /nfs_share/v4h_whitehawk/lib/ld-linux*
```

For WhiteHawk:

```text
/nfs_share/v4h_whitehawk/lib/ld-linux-aarch64.so.1
```

## Check startup objects

```bash
ls -l /nfs_share/v4h_whitehawk/usr/lib/crt*.o
```

---

# 4. Important Discovery: Target Rootfs GCC Is Not Host-Executable

The WhiteHawk rootfs contained:

```text
/nfs_share/v4h_whitehawk/usr/bin/aarch64-poky-linux-gcc
/nfs_share/v4h_whitehawk/usr/bin/aarch64-poky-linux-g++
/nfs_share/v4h_whitehawk/usr/bin/aarch64-poky-linux-ld
/nfs_share/v4h_whitehawk/usr/bin/aarch64-poky-linux-as
...
```

It also contained:

```text
/nfs_share/v4h_whitehawk/usr/lib/gcc/aarch64-poky-linux/9.3.0/
```

However:

```bash
file /nfs_share/v4h_whitehawk/usr/bin/aarch64-poky-linux-gcc
```

reported:

```text
ELF 64-bit LSB executable, ARM aarch64
```

and attempting to execute it on Ubuntu produced:

```text
zsh: exec format error
```

Therefore this GCC was a **target-side compiler executable**, not an x86_64-hosted cross compiler.

Do not try to execute it directly on the x86_64 development machine.

---

# 5. The Initial Approach That Failed

The initial linker wrapper was:

```bash
#!/bin/sh

exec /usr/bin/aarch64-linux-gnu-gcc \
    --sysroot=/nfs_share/v4h_whitehawk \
    "$@"
```

Cargo configuration:

```toml
[target.aarch64-unknown-linux-gnu]
linker = "scripts/whitehawk-linker.sh"
```

The build succeeded, but the resulting binary required:

```text
GLIBC_2.32
GLIBC_2.33
GLIBC_2.34
```

WhiteHawk only provided older glibc symbols.

Running the binary on WhiteHawk produced errors such as:

```text
DeviceAgent: /lib/libc.so.6: version `GLIBC_2.32' not found
DeviceAgent: /lib/libc.so.6: version `GLIBC_2.33' not found
DeviceAgent: /lib/libc.so.6: version `GLIBC_2.34' not found
```

The reason was that Ubuntu's cross compiler was still selecting its own GCC/cross-library environment.

---

# 6. Investigating GCC Search Paths

The Ubuntu compiler:

```text
/usr/bin/aarch64-linux-gnu-gcc
```

reported its own libraries even when `--sysroot` was supplied.

For example:

```bash
/usr/bin/aarch64-linux-gnu-gcc \
    --sysroot=/nfs_share/v4h_whitehawk \
    -print-file-name=libc.so.6
```

returned something under:

```text
/usr/lib/gcc-cross/aarch64-linux-gnu/11/...
```

Similarly:

```bash
/usr/bin/aarch64-linux-gnu-gcc \
    --sysroot=/nfs_share/v4h_whitehawk \
    -print-file-name=libgcc_s.so.1
```

and:

```bash
/usr/bin/aarch64-linux-gnu-gcc \
    --sysroot=/nfs_share/v4h_whitehawk \
    -print-file-name=crt1.o
```

still resolved to the Ubuntu cross-toolchain.

Therefore:

> `--sysroot` alone is not necessarily sufficient when using a GCC cross compiler whose own GCC runtime/startup search paths take precedence.

---

# 7. Successful Solution

The solution was to explicitly provide the target's:

* startup object directory
* target library directories
* target sysroot

using both `-B` and `-L`.

The successful linker command was conceptually:

```bash
/usr/bin/aarch64-linux-gnu-gcc \
    --sysroot=/nfs_share/v4h_whitehawk \
    -B/nfs_share/v4h_whitehawk/usr/lib/ \
    -B/nfs_share/v4h_whitehawk/lib/ \
    -L/nfs_share/v4h_whitehawk/usr/lib \
    -L/nfs_share/v4h_whitehawk/lib \
    ...
```

A link trace showed that the critical components were then taken from the WhiteHawk rootfs:

```text
/nfs_share/v4h_whitehawk/usr/lib/Scrt1.o
/nfs_share/v4h_whitehawk/usr/lib/crti.o
/nfs_share/v4h_whitehawk/lib/libgcc_s.so
/nfs_share/v4h_whitehawk/usr/lib/libc.so
/nfs_share/v4h_whitehawk/lib/libc.so.6
/nfs_share/v4h_whitehawk/usr/lib/libc_nonshared.a
/nfs_share/v4h_whitehawk/lib/ld-linux-aarch64.so.1
/nfs_share/v4h_whitehawk/usr/lib/crtn.o
```

Some GCC 11 compiler-runtime objects such as:

```text
/usr/lib/gcc-cross/aarch64-linux-gnu/11/crtbeginS.o
/usr/lib/gcc-cross/aarch64-linux-gnu/11/crtendS.o
/usr/lib/gcc-cross/aarch64-linux-gnu/11/libgcc.a
```

were still used.

However, the final binary did not inherit newer Ubuntu glibc requirements because the actual libc and dynamic linker came from WhiteHawk.

---

# 8. Final Working WhiteHawk Linker Script

Create:

```text
scripts/whitehawk-linker.sh
```

with:

```bash
#!/bin/sh

SYSROOT=/nfs_share/v4h_whitehawk
GCC=/usr/bin/aarch64-linux-gnu-gcc

exec "$GCC" \
    --sysroot="$SYSROOT" \
    -B"$SYSROOT/usr/lib/" \
    -B"$SYSROOT/lib/" \
    -L"$SYSROOT/usr/lib" \
    -L"$SYSROOT/lib" \
    "$@"
```

Make executable:

```bash
chmod +x scripts/whitehawk-linker.sh
```

---

# 9. Cargo Configuration

`.cargo/config.toml`:

```toml
[target.aarch64-unknown-linux-gnu]
linker = "scripts/whitehawk-linker.sh"
```

The Rust target is:

```text
aarch64-unknown-linux-gnu
```

Make sure it is installed:

```bash
rustup target add aarch64-unknown-linux-gnu
```

---

# 10. Build WhiteHawk DeviceAgent

From the project:

```bash
cd /home/hpcbdc/RacerFiles/eagent
```

Clean:

```bash
cargo clean
```

Build:

```bash
cargo build \
    --release \
    --target aarch64-unknown-linux-gnu
```

Output:

```text
target/aarch64-unknown-linux-gnu/release/edgeagent-rs
```

Rename/copy:

```bash
cp target/aarch64-unknown-linux-gnu/release/edgeagent-rs DeviceAgent
```

---

# 11. Verify the Binary

## Architecture

```bash
file DeviceAgent
```

Expected:

```text
ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV),
dynamically linked,
interpreter /lib/ld-linux-aarch64.so.1,
...
```

## Dynamic loader

```bash
readelf -l DeviceAgent | grep interpreter
```

Expected:

```text
/lib/ld-linux-aarch64.so.1
```

## GLIBC requirements

Always run:

```bash
readelf --version-info DeviceAgent |
    grep -o 'GLIBC_[0-9.]*' |
    sort -Vu
```

The successful WhiteHawk build produced:

```text
GLIBC_2.17
GLIBC_2.18
GLIBC_2.25
GLIBC_2.28
GLIBC_2.29
GLIBC_2.30
```

This was compatible with the WhiteHawk target.

## Dynamic dependencies

```bash
readelf -d DeviceAgent | grep NEEDED
```

---

# 12. Deploy to WhiteHawk

Copy:

```bash
scp DeviceAgent <user>@<whitehawk-ip>:/tmp/
```

On WhiteHawk:

```bash
chmod +x /tmp/DeviceAgent
```

Run:

```bash
/tmp/DeviceAgent
```

The successful build was verified by running it on the actual WhiteHawk target.

---

# 13. Generic Procedure for Another ARM64 Device

Suppose another device's rootfs is available at:

```text
/nfs_share/device_b
```

Do not reuse:

```text
/nfs_share/v4h_whitehawk
```

Use the new device's rootfs as the sysroot.

First inspect:

```bash
file /nfs_share/device_b/lib/libc.so.6
```

Check glibc:

```bash
strings /nfs_share/device_b/lib/libc.so.6 |
    grep -o 'GLIBC_[0-9.]*' |
    sort -Vu
```

Check dynamic loader:

```bash
ls -l /nfs_share/device_b/lib/ld-linux*
```

Check startup objects:

```bash
find /nfs_share/device_b/usr/lib \
    -maxdepth 1 \
    -name 'crt*.o' \
    -o -name 'Scrt*.o'
```

---

# 14. Generic ARM64 Linker Script

For another ARM64 glibc device:

```bash
#!/bin/sh

SYSROOT=/nfs_share/device_b
GCC=/usr/bin/aarch64-linux-gnu-gcc

exec "$GCC" \
    --sysroot="$SYSROOT" \
    -B"$SYSROOT/usr/lib/" \
    -B"$SYSROOT/lib/" \
    -L"$SYSROOT/usr/lib" \
    -L"$SYSROOT/lib" \
    "$@"
```

Then:

```bash
chmod +x scripts/device-b-linker.sh
```

Cargo:

```toml
[target.aarch64-unknown-linux-gnu]
linker = "scripts/device-b-linker.sh"
```

Build:

```bash
cargo clean
cargo build --release --target aarch64-unknown-linux-gnu
```

Verify:

```bash
file target/aarch64-unknown-linux-gnu/release/edgeagent-rs

readelf --version-info \
    target/aarch64-unknown-linux-gnu/release/edgeagent-rs |
    grep -o 'GLIBC_[0-9.]*' |
    sort -Vu
```

---

# 15. Better Generic Linker Script

For multiple target devices, avoid creating one script per device.

Use:

```text
scripts/cross-linker.sh
```

```bash
#!/bin/sh

set -eu

SYSROOT="${DEVICE_SYSROOT:?DEVICE_SYSROOT must be set}"
GCC="${CROSS_GCC:-/usr/bin/aarch64-linux-gnu-gcc}"

exec "$GCC" \
    --sysroot="$SYSROOT" \
    -B"$SYSROOT/usr/lib/" \
    -B"$SYSROOT/lib/" \
    -L"$SYSROOT/usr/lib" \
    -L"$SYSROOT/lib" \
    "$@"
```

Make executable:

```bash
chmod +x scripts/cross-linker.sh
```

Cargo:

```toml
[target.aarch64-unknown-linux-gnu]
linker = "scripts/cross-linker.sh"
```

Build WhiteHawk:

```bash
DEVICE_SYSROOT=/nfs_share/v4h_whitehawk \
cargo build --release --target aarch64-unknown-linux-gnu
```

Build another device:

```bash
DEVICE_SYSROOT=/nfs_share/device_b \
cargo build --release --target aarch64-unknown-linux-gnu
```

This allows the same Rust source tree to target multiple ARM64 devices.

---

# 16. Recommended Project Layout

For a project supporting multiple target devices:

```text
eagent/
├── .cargo/
│   └── config.toml
├── scripts/
│   ├── cross-linker.sh
│   ├── whitehawk-linker.sh
│   ├── ironhide-linker.sh
│   └── ...
├── src/
├── Cargo.toml
└── ...
```

If using the generic script, only:

```text
scripts/cross-linker.sh
```

is necessary.

---

# 17. Different CPU Architectures

The Rust target must match the target architecture and libc.

## ARM64 / AArch64 + glibc

Typical target:

```text
aarch64-unknown-linux-gnu
```

Typical compiler:

```text
aarch64-linux-gnu-gcc
```

This is the configuration used by WhiteHawk.

Examples:

```text
R-Car V4H
ARM64 Yocto
ARM64 Debian
ARM64 Ubuntu
```

---

## ARM32 + glibc

Typical target:

```text
arm-unknown-linux-gnueabihf
```

Typical compiler:

```text
arm-linux-gnueabihf-gcc
```

The same sysroot principle applies.

---

## x86_64

If development and target machines use compatible glibc versions:

```bash
cargo build --release
```

may be sufficient.

If the target has an older glibc, use the target rootfs as a sysroot just as with ARM64.

---

## ARM64 + musl

Typical target:

```text
aarch64-unknown-linux-musl
```

This is different from:

```text
aarch64-unknown-linux-gnu
```

Do not use the glibc procedure blindly for a musl target.

---

# 18. Choosing the Correct Rust Target

The CPU architecture alone is not enough.

You need to consider:

```text
CPU architecture
+
OS
+
libc
+
ABI
```

Examples:

| Target               | Rust target                   |
| -------------------- | ----------------------------- |
| ARM64 Linux + glibc  | `aarch64-unknown-linux-gnu`   |
| ARM64 Linux + musl   | `aarch64-unknown-linux-musl`  |
| ARM32 Linux + glibc  | `arm-unknown-linux-gnueabihf` |
| x86_64 Linux + glibc | `x86_64-unknown-linux-gnu`    |

---

# 19. Why the Target Rootfs Is Important

Consider:

```text
Development Ubuntu:
glibc 2.39

Target:
glibc 2.31
```

If Rust is linked normally on Ubuntu, the resulting executable may contain requirements such as:

```text
GLIBC_2.34
GLIBC_2.35
GLIBC_2.36
...
```

The older target cannot execute it.

Using the target rootfs:

```text
Target rootfs
      |
      +-- libc.so.6
      +-- libc_nonshared.a
      +-- ld-linux-aarch64.so.1
      +-- libgcc_s.so.1
      +-- crt*.o
      +-- other target libraries
```

causes the linker to resolve against the target environment.

---

# 20. Always Check GLIBC Compatibility

This should be part of every cross-compilation workflow.

Run:

```bash
readelf --version-info DeviceAgent |
    grep -o 'GLIBC_[0-9.]*' |
    sort -Vu
```

Then compare the highest required version with the target's available versions:

```bash
strings /path/to/target-rootfs/lib/libc.so.6 |
    grep -o 'GLIBC_[0-9.]*' |
    sort -Vu
```

For example:

```text
Binary:
GLIBC_2.17
GLIBC_2.18
GLIBC_2.25
GLIBC_2.28
GLIBC_2.29
GLIBC_2.30

Target:
GLIBC_2.17
...
GLIBC_2.30
GLIBC_2.31
```

This is compatible.

If the binary requires:

```text
GLIBC_2.34
```

while the target only supports:

```text
GLIBC_2.31
```

the binary is not compatible.

---

# 21. Debugging Linker Problems

Use:

```bash
-Wl,-t
```

to see exactly which files the linker is using.

Example:

```bash
/usr/bin/aarch64-linux-gnu-gcc \
    --sysroot=/nfs_share/v4h_whitehawk \
    -B/nfs_share/v4h_whitehawk/usr/lib/ \
    -B/nfs_share/v4h_whitehawk/lib/ \
    -L/nfs_share/v4h_whitehawk/usr/lib \
    -L/nfs_share/v4h_whitehawk/lib \
    -Wl,-t \
    -xc -o /tmp/test-whitehawk \
    - <<'EOF'
int main(void) { return 0; }
EOF
```

Look for:

```text
/nfs_share/v4h_whitehawk/usr/lib/Scrt1.o
/nfs_share/v4h_whitehawk/usr/lib/crti.o
/nfs_share/v4h_whitehawk/lib/libgcc_s.so
/nfs_share/v4h_whitehawk/lib/libc.so.6
/nfs_share/v4h_whitehawk/lib/ld-linux-aarch64.so.1
/nfs_share/v4h_whitehawk/usr/lib/crtn.o
```

If you instead see Ubuntu libraries such as:

```text
/usr/aarch64-linux-gnu/...
/usr/lib/gcc-cross/aarch64-linux-gnu/...
```

investigate before deploying.

---

# 22. Important Distinction: `-B` vs `-L`

`-B` influences GCC's search for compiler/startup-related files, including files such as:

```text
crtbeginS.o
crtendS.o
Scrt1.o
crti.o
crtn.o
```

`-L` influences linker library search paths, such as:

```text
libc.so
libgcc_s.so
libm.so
```

Therefore the successful WhiteHawk configuration used both:

```text
-B target/usr/lib
-B target/lib
-L target/usr/lib
-L target/lib
```

along with:

```text
--sysroot=target
```

---

# 23. Preferred Long-Term Solution

The cleanest possible solution is to obtain the actual **host-side Yocto SDK/cross compiler** matching the target.

For example:

```text
x86_64 host
    |
    +-- aarch64-poky-linux-gcc 9.3
    |
    +-- matching Yocto sysroot
```

This avoids mixing:

```text
Ubuntu GCC 11
```

with:

```text
Yocto GCC 9.3
```

However, the WhiteHawk build demonstrated that the explicit sysroot/linker approach works successfully with the available Ubuntu cross compiler.

If a proper matching Yocto SDK is available, prefer it for production toolchain management.

---

# 24. WhiteHawk Final Working Configuration

For future reference, this is the exact configuration that successfully produced a runnable `DeviceAgent`.

## `.cargo/config.toml`

```toml
[target.aarch64-unknown-linux-gnu]
linker = "scripts/whitehawk-linker.sh"
```

## `scripts/whitehawk-linker.sh`

```bash
#!/bin/sh

SYSROOT=/nfs_share/v4h_whitehawk
GCC=/usr/bin/aarch64-linux-gnu-gcc

exec "$GCC" \
    --sysroot="$SYSROOT" \
    -B"$SYSROOT/usr/lib/" \
    -B"$SYSROOT/lib/" \
    -L"$SYSROOT/usr/lib" \
    -L"$SYSROOT/lib" \
    "$@"
```

## Build

```bash
cargo clean

cargo build \
    --release \
    --target aarch64-unknown-linux-gnu
```

## Rename

```bash
cp target/aarch64-unknown-linux-gnu/release/edgeagent-rs DeviceAgent
```

## Verify

```bash
file DeviceAgent
```

```bash
readelf -l DeviceAgent | grep interpreter
```

```bash
readelf --version-info DeviceAgent |
    grep -o 'GLIBC_[0-9.]*' |
    sort -Vu
```

```bash
readelf -d DeviceAgent | grep NEEDED
```

## Deploy

```bash
scp DeviceAgent <user>@<whitehawk-ip>:/tmp/
```

On WhiteHawk:

```bash
chmod +x /tmp/DeviceAgent
/tmp/DeviceAgent
```

---

# 25. Quick Checklist for Future Devices

When adding another device:

```text
[ ] Determine CPU architecture
[ ] Determine libc: glibc or musl
[ ] Determine target glibc version
[ ] Obtain/copy target rootfs
[ ] Verify target libc.so.6 architecture
[ ] Verify target dynamic linker
[ ] Verify target startup objects
[ ] Select matching Rust target
[ ] Select host-side cross compiler
[ ] Create/use linker wrapper
[ ] Use --sysroot
[ ] Use -B target/usr/lib
[ ] Use -B target/lib
[ ] Use -L target/usr/lib
[ ] Use -L target/lib
[ ] cargo clean
[ ] cargo build --release --target ...
[ ] file binary
[ ] check dynamic interpreter
[ ] check GLIBC_* requirements
[ ] check NEEDED libraries
[ ] copy to target
[ ] execute on target
```

---

# 26. Golden Rule

The most important rule from this entire exercise is:

> **For an old Yocto/embedded Linux target, never assume that a successful cross-compilation means the binary is deployable. Always inspect the binary's architecture, dynamic loader, dependencies, and GLIBC symbol requirements against the actual target rootfs.**

For the WhiteHawk case, the final successful chain was:

```text
Rust 1.98.1
      |
      v
aarch64-unknown-linux-gnu
      |
      v
/usr/bin/aarch64-linux-gnu-gcc
      |
      +-- --sysroot=/nfs_share/v4h_whitehawk
      +-- -B /nfs_share/v4h_whitehawk/usr/lib
      +-- -B /nfs_share/v4h_whitehawk/lib
      +-- -L /nfs_share/v4h_whitehawk/usr/lib
      +-- -L /nfs_share/v4h_whitehawk/lib
      |
      v
WhiteHawk target libraries
      |
      v
GLIBC requirements <= target capabilities
      |
      v
ARM64 DeviceAgent
      |
      v
R-Car V4H WhiteHawk
```

This procedure is now the baseline recipe for cross-compiling `edgeagent-rs`/`DeviceAgent` for the WhiteHawk and similar embedded Linux targets.
