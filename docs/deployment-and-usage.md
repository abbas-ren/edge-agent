# R-Car deployment and usage

## Install a cross-build bundle

Choose the bundle matching the configured generation and the target image's
SDK. Verify it before copying:

```sh
cd dist/gen4
sha256sum -c SHA256SUMS
file edgeagent-rs
```

On the target, stop and disable the legacy agent first:

```sh
systemctl disable --now dev-agent.service 2>/dev/null || true
install -m 0755 edgeagent-rs /usr/bin/edgeagent-rs
install -d -m 0700 /etc/edgeagent
install -m 0600 config.toml /etc/edgeagent/config.toml
install -m 0644 edgeagent-rs.service /usr/lib/systemd/system/edgeagent-rs.service
```

Provision `/mnt/emmc` through `/etc/fstab` before enabling the unit. The service
unit requires that mount to be writable and grants writes only to `/mnt/emmc`
and `/usr/src` under its protected filesystem view.

## Configure each generation

Edit `/etc/edgeagent/config.toml` on each target:

```toml
[farm]
base_url = "https://farm.example"
require_tls = true

[device]
interface = "eth0"
generation = 4
name = "rcar-v4m-01"
test_timeout_seconds = 3600
```

Use `generation = 3`, `4`, or `5` to match the board. The same binary supports
all three generations. If `device_type` is omitted, the agent detects known
board models from `/proc/device-tree/model`.

Generation-specific behavior:

| Generation | Agent behavior |
| --- | --- |
| Gen3 | Gen3 registration/inventory and legacy boot address layout |
| Gen4 | Gen4 registration/inventory and legacy boot address layout |
| Gen5 | Gen5 inventory template, Gen5 boot addresses, static IP bootargs, and reboot callback |

The EdgeController, not EdgeAgent, owns relay/GPIO control for Gen3/Gen4 and
CPLD power/download control for Gen5.

## Configure authentication

Do not store a production token in the bundle. Put it in a root-only environment
file:

```sh
cat >/etc/edgeagent/environment <<'EOF'
EDGEAGENT_API_TOKEN=replace-with-at-least-32-random-characters
EOF
chmod 0600 /etc/edgeagent/environment
```

Configure the same value as `EDGE_AGENT_TOKEN` for FarmController. This token is
used when FarmController proxies target logs and log-level changes.

## Required target facilities

- `/mnt/emmc` writable persistent mount.
- `/etc/fw_env.config` matching the board's U-Boot environment layout.
- `/usr/bin/fw_printenv` and `/usr/bin/fw_setenv` from `u-boot-fw-utils`.
- `/sbin/reboot` and `/bin/dmesg` at the configured paths.
- `/usr/src/tests/<testId>/<caseId>.sh` on the selected NFS root for tests.
- CA certificates and correct system time for HTTPS/WSS.
- Outbound access to FarmController and inbound TCP 8888 from FarmController.

## Validate and start

The configuration check reads the configured interface MAC and prepares the UID
file, so run it on the target rather than the build host:

```sh
/usr/bin/edgeagent-rs --config /etc/edgeagent/config.toml --check
systemd-analyze verify /usr/lib/systemd/system/edgeagent-rs.service
systemctl daemon-reload
systemctl enable --now edgeagent-rs.service
```

Inspect startup and registration:

```sh
systemctl status edgeagent-rs.service
journalctl -u edgeagent-rs.service -f
curl --fail http://127.0.0.1:8888/health
curl -i http://127.0.0.1:8888/ready
```

`/ready` returns `200` after the MAC-derived identity has been prepared.

## Normal operation

The agent atomically writes the configured interface MAC to the UID path without
separators or trailing bytes, registers itself with FarmController, then
maintains `/ws?deviceId=<MAC_WITHOUT_COLONS>`. Heartbeats contain structured
hardware inventory. FarmController approval validates the canonical device ID.

Administrators view target logs through the frontend Logs page:

1. Select `EdgeAgent` as the source.
2. Select the approved target device.
3. Search the bounded log buffer or change its runtime capture level.

The browser communicates only with FarmController. FarmController resolves the
approved device IP and proxies `/logs` or `/logs/level` to TCP 8888.

## Upgrade and rollback

Keep the previous binary until the new service passes its checks:

```sh
systemctl stop edgeagent-rs.service
cp -a /usr/bin/edgeagent-rs /usr/bin/edgeagent-rs.previous
install -m 0755 edgeagent-rs /usr/bin/edgeagent-rs
/usr/bin/edgeagent-rs --config /etc/edgeagent/config.toml --check
systemctl start edgeagent-rs.service
```

Rollback if startup or FarmController connectivity fails:

```sh
systemctl stop edgeagent-rs.service
mv /usr/bin/edgeagent-rs.previous /usr/bin/edgeagent-rs
systemctl start edgeagent-rs.service
```

Do not run the Rust and legacy agents simultaneously. Preserve `/mnt/emmc` when
upgrading so approval identity and heartbeat settings survive.