# EdgeAgent Rust

`edgeagent-rs` is the target-board service for FarmController. It replaces the
legacy C `DeviceAgent` while preserving its HTTP and FarmController contracts.

## Capabilities

- Registers at `POST /api/v1/device/` before opening its heartbeat session.
- Prepares the configured UID file from the interface MAC at every startup.
- Sends JSON-object inventory heartbeats to `/ws?deviceId=<MAC_WITHOUT_COLONS>`.
- Serves the compatibility API on port `8888`.
- Configures U-Boot with direct `fw_setenv` argument calls; no shell is used.
- Runs bounded test processes under `/usr/src/tests/<testId>` and reports each
  completed numeric testcase ID over the FarmController WebSocket.
- Persists approval, heartbeat timeout, and reboot-spanning test state atomically.
- Exposes bounded structured logs and runtime log-level control for the admin UI.
- Uses Rustls for outbound HTTPS and WSS connections.

## Required target setup

Install these files and tools:

| Path | Purpose |
| --- | --- |
| `/usr/bin/edgeagent-rs` | Agent binary |
| `/etc/edgeagent/config.toml` | Strict service configuration |
| `/mnt/emmc` | Writable persistent mount |
| `/mnt/emmc/UID.txt` | MAC-derived device ID, prepared by the agent without separators or trailing bytes |
| `/mnt/emmc/timeout` | Runtime heartbeat interval, created by the agent |
| `/usr/src/tests/<testId>/<caseId>.sh` | Executable testcases from the NFS image |
| `/etc/fw_env.config` | Valid U-Boot environment layout for the board |
| `/usr/bin/fw_printenv` | Read U-Boot build version |
| `/usr/bin/fw_setenv` | Update verified boot variables |
| `/sbin/reboot` | Reboot target after a root-filesystem change |
| `/bin/dmesg` | Capture kernel output after each testcase |

The service runs as root because U-Boot, reboot, kernel logs, raw hardware
inventory, and NFS test artifacts require privileges on the current images.
The systemd unit restricts filesystem writes to `/mnt/emmc` and `/usr/src`.

Mount the eMMC partition through `/etc/fstab`; do not mount it in a shell
wrapper. For the legacy layout this is typically `/dev/mmcblk0p1 /mnt/emmc`.

## Configuration

Start from [config/default.toml](config/default.toml). Set:

- `farm.base_url` to FarmController's externally reachable origin, normally
  `https://<farm-host>` through the deployment reverse proxy.
- `device.interface` to the interface whose MAC and IPv4 address identify the
  target.
- `device.generation` to `3`, `4`, or `5`.
- `security.api_token` to a random 32-256 character value.

Set the same secret as `EDGE_AGENT_TOKEN` in the FarmController service. Keep
the token out of source-controlled TOML by placing it in
`/etc/edgeagent/environment`:

```ini
EDGEAGENT_API_TOKEN=replace-with-a-random-secret-of-at-least-32-characters
```

If no token is configured, the API accepts only loopback or the literal IPv4
host from `farm.base_url`. A token is required when FarmController is addressed
by DNS. Cleartext `http://` is rejected unless `farm.require_tls = false`, which
is intended only for isolated development networks.

When the TOML file does not exist, the agent can read the legacy
`/etc/config/login.cfg` keys `server_ip`, `http_port`, `ws_port`, `iface_name`,
and `gen`. New deployments should use TOML.

Validate before starting:

```sh
/usr/bin/edgeagent-rs --config /etc/edgeagent/config.toml --check
```

## HTTP API

All control and log routes require `Authorization: Bearer <token>` when a token
is configured. Without a token, requests are source-IP allowlisted.

| Method | Route | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Process health and reboot state |
| `GET` | `/ready` | Local identity readiness |
| `GET` | `/status` | Identity, generation, and active test |
| `POST` | `/approve` | Validate and confirm `{"UID":"aabbccddeeff"}` |
| `POST` | `/delete` | Remove approval and heartbeat persistence |
| `POST` | `/configure/heartbeat` | Persist and immediately apply `{"timeout":5}` |
| `POST` | `/flash` | Validate and configure the requested NFS boot |
| `POST` | `/test-execution` | Reboot to the NFS image or run the test |
| `POST` | `/cancel` | Cancel the matching active `testId` |
| `GET` | `/logs?limit=500` | Read bounded structured logs |
| `PUT` | `/logs/level` | Apply `{"level":"debug"}` without restart |

Flash and test requests retain the legacy shape:

```json
{
  "nfs": "/exports/boards/build-123",
  "image": "/images/build-123/Image",
  "dtb": "/images/build-123/board.dtb",
  "server_ip": "192.0.2.10",
  "testId": "execution-42",
  "build_ver": "build-123"
}
```

Remote paths and IDs are allowlisted before they reach process arguments. Test
scripts execute directly, have a configurable timeout, and cannot escape the
configured test root. Output is written to `<caseId>.output`, kernel logs to
`<caseId>.txt`, and the output footer records the exit code plus `PASS`/`FAIL`.

## Install with systemd

```sh
install -m 0755 target/release/edgeagent-rs /usr/bin/edgeagent-rs
install -d -m 0700 /etc/edgeagent
install -m 0600 config/default.toml /etc/edgeagent/config.toml
install -m 0644 packaging/systemd/edgeagent-rs.service /usr/lib/systemd/system/
systemctl daemon-reload
systemctl enable --now edgeagent-rs.service
```

Disable the old `DeviceAgent`/`dev-agent.service` first so only one process owns
port `8888` and the FarmController heartbeat session.

## Yocto

The template at `packaging/yocto/edgeagent-rs_0.1.0.bb` expects these files in
the recipe's `files/` directory:

- A reproducible `edgeagent-rs-0.1.0.tar.zst` source archive.
- `edgeagent-rs.service`.
- A board-specific `config.toml` with no embedded production secret.

Generate and check in the crate source/vendor metadata according to the target
layer's standard Cargo workflow, then add `edgeagent-rs` to the image and remove
the legacy `dev-agent` and startup-script packages. Retain `u-boot-fw-utils` and
ensure `/mnt/emmc` is mounted before the unit starts.

## Cross-compilation and operations

- [Cross-compiling for R-Car](docs/cross-compilation.md) covers standalone
  Yocto SDK builds for Gen3, Gen4, and Gen5, target inference, bundle contents,
  and the full BitBake path.
- [R-Car deployment and usage](docs/deployment-and-usage.md) covers target
  installation, generation configuration, authentication, startup, normal
  operation, upgrades, and rollback.

Show the standalone builder options or validate a matrix without compiling:

```sh
scripts/cross-build.sh --help
scripts/cross-build.sh --generation all --target aarch64-unknown-linux-gnu --dry-run
```

## Development validation

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```