SUMMARY = "Rust Farm target-device agent"
DESCRIPTION = "Secure target registration, heartbeat, inventory, flashing, test execution, and runtime logs"
LICENSE = "CLOSED"

SRC_URI = "file://edgeagent-rs-${PV}.tar.zst"
SRC_URI += "file://edgeagent-rs.service"
SRC_URI += "file://config.toml"

S = "${WORKDIR}/edgeagent-rs-${PV}"

inherit cargo systemd

CARGO_SRC_DIR = "${S}"
SYSTEMD_SERVICE:${PN} = "edgeagent-rs.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

RDEPENDS:${PN} += "u-boot-fw-utils util-linux-mount"

do_compile() {
    oe_cargo_build --release --locked
}

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${B}/target/${RUST_HOST_SYS}/release/edgeagent-rs ${D}${bindir}/edgeagent-rs

    install -d ${D}${sysconfdir}/edgeagent
    install -m 0600 ${WORKDIR}/config.toml ${D}${sysconfdir}/edgeagent/config.toml

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${WORKDIR}/edgeagent-rs.service ${D}${systemd_system_unitdir}/edgeagent-rs.service

    install -d ${D}/mnt/emmc ${D}/usr/src/tests
}

FILES:${PN} += "${sysconfdir}/edgeagent ${systemd_system_unitdir}/edgeagent-rs.service /mnt/emmc /usr/src/tests"