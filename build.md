# R-Car Gen4 Gray Hawk Cross-Compilation

This guide builds `edgeagent-rs` for the R-Car Gen4 V4M Gray Hawk target with
the SDK generated from the **same Renesas Yocto BSP configuration and image**
that runs on the board.

The procedure follows the Yocto Project's
[Obtaining the SDK](https://docs.yoctoproject.org/sdk-manual/appendix-obtain.html)
guidance: build a standard SDK installer with `populate_sdk`, install it, and
use its `environment-setup-*` file and target sysroot.

## Why the exact SDK is required

An AArch64 binary is not automatically compatible with every AArch64 root
filesystem. A dynamically linked Rust binary must not require a newer glibc
than the target image provides. A generic host cross-compiler can therefore
produce a valid AArch64 ELF that fails on Gray Hawk with errors such as:

```text
version `GLIBC_2.34' not found
```

Do not copy `libc.so.6` or `ld-linux-aarch64.so.1` from Gen3 or another image.
`GLIBC_PRIVATE` symbols are private contracts between the loader and libc;
mixing those files can make every dynamically linked command on the target
unusable.

## Inputs

Collect these values from the build that produced the deployed Gray Hawk
rootfs. Do not guess them from the CPU architecture or kernel version.

```sh
export GEN4_BSP_DIR=/path/to/renesas-gen4-bsp
export GEN4_BUILD_DIR=/path/to/renesas-gen4-bsp/build-grayhawk
export GEN4_IMAGE=<exact-image-recipe>
export SDK_INSTALL_DIR="$HOME/sdk/rcar-gen4-grayhawk"
```

`GEN4_IMAGE` is the BitBake image target used for the board, without an output
extension. The build directory's `conf/local.conf` must contain the Gray Hawk
`MACHINE` selected by the Renesas BSP. This repository does not contain that BSP
configuration, so this guide intentionally does not prescribe a machine name.

The build host also needs:

- A supported Linux distribution and the packages required by the Renesas BSP.
- Substantial free storage for the BSP build, shared-state cache, downloads,
  and installed SDK; use the capacity required by that BSP release rather than
  assuming the SDK alone represents the full build footprint.
- Outbound access for Yocto sources and Rust crates, or pre-populated download,
  shared-state, and Cargo caches.
- Bash 4 or newer, `file`, `readelf`, `sha256sum`, and Rust 1.85 or newer. Rust
  1.85 is the first stable release supporting the project's 2024 edition.

## 1. Confirm the target image

On a healthy or restored target, record the userspace version:

```sh
uname -m
getconf GNU_LIBC_VERSION
cat /etc/os-release
```

Expected architecture is `aarch64`. Preserve the output with the image/build
metadata. If the target currently reports `_dl_audit_symbind_alt` or another
`GLIBC_PRIVATE` lookup error, restore or reflash the exact Gray Hawk rootfs
before deploying EdgeAgent. Building EdgeAgent does not repair a mixed libc and
dynamic loader.

In the Yocto build environment, verify the selected machine and image before
creating the SDK:

```sh
cd "$GEN4_BSP_DIR"
# Use the Renesas BSP's documented environment setup command. Common BSPs use
# one of the following forms; use only the form supplied by your BSP release.
# source oe-init-build-env "$GEN4_BUILD_DIR"
# source setup-environment "$GEN4_BUILD_DIR"

test -f "$GEN4_BUILD_DIR/conf/local.conf"
grep -E '^[[:space:]]*MACHINE([[:space:]:?+]|$)' "$GEN4_BUILD_DIR/conf/local.conf" || true
bitbake -e "$GEN4_IMAGE" | sed -n 's/^MACHINE="\([^"]*\)"/Resolved MACHINE: \1/p'
bitbake -e "$GEN4_IMAGE" | sed -n 's/^SDKMACHINE="\([^"]*\)"/SDK host architecture: \1/p'
```

Stop if the resolved machine, BSP branch, distro configuration, or image target
differs from the deployed board image.

## 2. Build the standard Yocto SDK installer

From the initialized Gray Hawk BitBake environment:

```sh
bitbake "$GEN4_IMAGE" -c populate_sdk
```

Yocto writes the installer under the build directory's `tmp/deploy/sdk`:

```sh
export GEN4_DEPLOY_DIR_SDK="$(bitbake -e "$GEN4_IMAGE" | sed -n 's/^DEPLOY_DIR_SDK="\([^"]*\)"/\1/p')"
test -n "$GEN4_DEPLOY_DIR_SDK"
find "$GEN4_DEPLOY_DIR_SDK" -maxdepth 1 -type f -name '*.sh' -print
```

Select the installer produced by this command, not a generic Poky/QEMU SDK from
the Yocto download server. Pre-built Yocto SDKs contain sysroots for their own
reference images, not the Renesas Gray Hawk image.

A standard SDK is sufficient for the standalone Cargo build. An extensible SDK
can be generated with `populate_sdk_ext`, but it is not required by
`scripts/cross-build.sh`.

## 3. Install the SDK

Run the generated installer as the build user:

```sh
SDK_INSTALLER="$GEN4_DEPLOY_DIR_SDK/<generated-sdk-installer>.sh"
chmod u+x "$SDK_INSTALLER"
"$SDK_INSTALLER"
```

When prompted, use the value of `SDK_INSTALL_DIR`. Installing below the home
directory avoids requiring root privileges. Locate the installed environment
file:

```sh
find "$SDK_INSTALL_DIR" -maxdepth 3 -type f -name 'environment-setup-*' -print
export RCAR_GEN4_SDK_ENV="$SDK_INSTALL_DIR/<version>/environment-setup-<target>-linux"
test -f "$RCAR_GEN4_SDK_ENV"
```

The exact installed path and target tuple are generated by the BSP. Replace the
placeholder with the path printed by `find`.

## 4. Verify the SDK and sysroot

Inspect the environment in a subshell so it does not pollute the shell used by
Rustup:

```sh
(
  set -eu
  . "$RCAR_GEN4_SDK_ENV"
  compiler=${CC%% *}
  printf 'CC=%s\n' "$CC"
  printf 'AR=%s\n' "${AR:-<unset>}"
  printf 'SDKTARGETSYSROOT=%s\n' "$SDKTARGETSYSROOT"
  test -x "$compiler"
  test -d "$SDKTARGETSYSROOT"
  "$compiler" --version
  "$compiler" --print-sysroot
)
```

The compiler must target AArch64 and `SDKTARGETSYSROOT` must point inside this
installed SDK. If the compiler or sysroot belongs to another BSP release, do not
continue.

## 5. Prepare Rust

From the `edgeagent-rs` repository on the Linux build host:

```sh
rustc --version  # Must be 1.85 or newer for edition 2024.
rustup target add aarch64-unknown-linux-gnu
cargo --version
```

Installing the Rust target provides Rust's target standard library. It does not
provide the C linker, glibc, or sysroot; those come from the Gray Hawk SDK.

Run the host-side checks before cross-compiling:

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

## 6. Cross-compile the Gen4 bundle

First validate target inference without compiling:

```sh
scripts/cross-build.sh \
  --generation 4 \
  --sdk-env "$RCAR_GEN4_SDK_ENV" \
  --dry-run
```

Then build the release bundle:

```sh
scripts/cross-build.sh \
  --generation 4 \
  --sdk-env "$RCAR_GEN4_SDK_ENV"
```

The script sources the SDK in an isolated subshell, uses its `CC`, `AR`,
`LDFLAGS`, and `SDKTARGETSYSROOT`, and writes:

```text
dist/gen4/
├── edgeagent-rs
├── config.toml
├── edgeagent-rs.service
├── build-manifest.txt
└── SHA256SUMS
```

Do not add `--allow-generic` for a deployable Gray Hawk build. That option is
only an explicit escape hatch for development artifacts built without the
board SDK.

## 7. Validate ABI compatibility

Validate the bundle on the build host:

```sh
cd dist/gen4
sha256sum -c SHA256SUMS
file edgeagent-rs
readelf --program-headers edgeagent-rs | grep 'Requesting program interpreter'
readelf --version-info edgeagent-rs \
  | grep -o 'GLIBC_[0-9.]*' \
  | sort -Vu
cat build-manifest.txt
```

Expected properties:

- ELF architecture is AArch64.
- Interpreter is normally `/lib/ld-linux-aarch64.so.1` for the GNU target.
- `sdk_environment` is the installed Gray Hawk SDK environment file.
- `sdk_sysroot` is not `none` and belongs to that SDK.
- `compiler` is the SDK compiler, not generic `aarch64-linux-gnu-gcc`.
- `linkage=dynamic` for the standard glibc SDK build.
- Every listed `glibc_requirements` version is available in the target rootfs.

The strongest compatibility check is to build the SDK from the exact image.
For an additional check, compare the highest required symbol with the restored
target:

```sh
# Build host
readelf --version-info edgeagent-rs \
  | grep -o 'GLIBC_[0-9.]*' \
  | sort -Vu \
  | tail -n 1

# Gray Hawk target
getconf GNU_LIBC_VERSION
```

Do not deploy if the binary requires a newer version.

## 8. Configure the Gen4 bundle

Edit `dist/gen4/config.toml` before deployment:

```toml
[farm]
base_url = "https://farm.example"
require_tls = true

[device]
interface = "eth0"
generation = 4
name = "rcar-v4m-grayhawk-01"
test_timeout_seconds = 3600
```

Do not store a production API token in this file. Provision
`EDGEAGENT_API_TOKEN` through the root-only systemd environment file described
in [deployment and usage](docs/deployment-and-usage.md).

## 9. Deploy only to a healthy target

Before copying the bundle, verify that normal dynamic commands work on Gray
Hawk:

```sh
/bin/true
getconf GNU_LIBC_VERSION
/lib/ld-linux-aarch64.so.1 --help >/dev/null
```

If any command reports `GLIBC_PRIVATE`, restore/reflash the target rootfs first.
Do not attempt to repair it with libraries from Gen3.

Copy the verified bundle to a staging directory, verify it there, and install:

```sh
# Build host
ssh root@<grayhawk-ip> 'mkdir -p /tmp/edgeagent-gen4'
scp dist/gen4/edgeagent-rs \
    dist/gen4/config.toml \
    dist/gen4/edgeagent-rs.service \
    dist/gen4/build-manifest.txt \
    dist/gen4/SHA256SUMS \
    root@<grayhawk-ip>:/tmp/edgeagent-gen4/
```

```sh
# Gray Hawk target
cd /tmp/edgeagent-gen4
sha256sum -c SHA256SUMS
./edgeagent-rs --config ./config.toml --check

systemctl disable --now dev-agent.service 2>/dev/null || true
install -m 0755 edgeagent-rs /usr/bin/edgeagent-rs
install -d -m 0700 /etc/edgeagent
install -m 0600 config.toml /etc/edgeagent/config.toml
install -m 0644 edgeagent-rs.service /usr/lib/systemd/system/edgeagent-rs.service
systemctl daemon-reload
systemctl enable --now edgeagent-rs.service
```

Validate startup:

```sh
systemctl status edgeagent-rs.service
journalctl -u edgeagent-rs.service -n 100 --no-pager
curl --fail http://127.0.0.1:8888/health
curl -i http://127.0.0.1:8888/ready
```

## Full Yocto image alternative

For production images, the stronger integration path is to add
[the recipe template](packaging/yocto/edgeagent-rs_0.1.0.bb) to the Renesas BSP
layer, provide its vendored Cargo source archive and configuration files, add
`edgeagent-rs` to `IMAGE_INSTALL`, and build the complete image with BitBake.
This keeps compilation, runtime dependencies, packaging, and rootfs generation
inside one BSP build.

The template requires `u-boot-fw-utils` and `util-linux-mount`. Remove the
legacy `dev-agent` and startup-script packages so two agents do not bind TCP
port 8888.

## Troubleshooting

### `GLIBC_x.y not found`

The binary was linked against a newer sysroot than the target. Confirm that the
manifest names the Gray Hawk SDK, regenerate the SDK from the exact deployed
image, and rebuild. Do not update individual target libc files.

### `GLIBC_PRIVATE` or `_dl_audit_symbind_alt` not found

The target's libc and dynamic loader are from different builds. EdgeAgent cannot
fix this. Restore both through the exact Gray Hawk rootfs/package or reflash the
image before testing any new binary.

### Cross-linker not found

The SDK environment file is wrong or was not installed completely. Run:

```sh
scripts/cross-build.sh --list-sdks
```

Then pass the actual `environment-setup-*` path. `rustup target add` does not
install a linker.

### Cargo cannot fetch crates

Populate Cargo's cache before an isolated build, or use the BSP recipe with
vendored sources. For a previously populated cache:

```sh
scripts/cross-build.sh \
  --generation 4 \
  --sdk-env "$RCAR_GEN4_SDK_ENV" \
  --offline
```

### Wrong architecture or interpreter

Stop deployment. Recheck the BSP `MACHINE`, SDK installer, inferred Rust target,
and manifest. Do not override the Rust target until the SDK compiler tuple is
understood.

## References

- [Yocto Project: Obtaining the SDK](https://docs.yoctoproject.org/sdk-manual/appendix-obtain.html)
- [Cross-compiling for R-Car](docs/cross-compilation.md)
- [R-Car deployment and usage](docs/deployment-and-usage.md)
- [Cross-build script](scripts/cross-build.sh)
