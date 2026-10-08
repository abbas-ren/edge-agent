use std::{collections::BTreeMap, fs, net::Ipv4Addr, path::Path};

use nix::ifaddrs::getifaddrs;
use serde_json::{Map, Value, json};

use crate::config::Config;

pub fn ip_address(interface: &str) -> anyhow::Result<Ipv4Addr> {
    for address in getifaddrs()? {
        if address.interface_name == interface
            && let Some(socket_address) = address.address
            && let Some(ipv4) = socket_address.as_sockaddr_in()
        {
            return Ok(ipv4.ip());
        }
    }
    anyhow::bail!("selected interface has no usable IPv4 address: {interface}")
}

pub fn snapshot(config: &Config) -> Value {
    if config.device.generation == 5 {
        let path = Path::new("/etc/config/gen5_interface.json");
        if let Ok(bytes) = fs::read(path)
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            return value;
        }
    }
    let mut root = Map::new();
    root.insert("network".into(), network());
    root.insert("usb".into(), usb_devices());
    root.insert(
        "serial".into(),
        named_entries("/sys/class/tty", |name| {
            name.starts_with("ttyUSB") || name.starts_with("ttyACM")
        }),
    );
    root.insert("pci".into(), pci_devices());
    root.insert(
        "audio".into(),
        named_entries("/sys/class/sound", |name| name.starts_with("card")),
    );
    root.insert("display".into(), display_connectors(|_| true));
    root.insert(
        "lvds".into(),
        display_connectors(|name| name.to_ascii_lowercase().contains("lvds")),
    );
    root.insert(
        "camera".into(),
        named_entries("/sys/class/video4linux", |_| true),
    );
    root.insert(
        "can".into(),
        network_entries(|name| name.starts_with("can")),
    );
    root.insert(
        "i2c".into(),
        named_entries("/sys/bus/i2c/devices", |_| true),
    );
    root.insert("temperature".into(), temperatures());
    root.insert("block".into(), named_entries("/sys/class/block", |_| true));
    root.insert(
        "sata".into(),
        named_entries("/sys/class/block", |name| name.starts_with("sd")),
    );
    Value::Object(root)
}

pub fn registration_interfaces(snapshot: &Value) -> BTreeMap<String, Vec<String>> {
    snapshot
        .as_object()
        .into_iter()
        .flat_map(|root| root.iter())
        .map(|(kind, value)| {
            let names = value
                .as_object()
                .map(|entries| {
                    entries
                        .keys()
                        .filter(|key| key.as_str() != "not present")
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            (kind.clone(), names)
        })
        .collect()
}

fn network() -> Value {
    network_entries(|_| true)
}

fn network_entries(include: impl Fn(&str) -> bool) -> Value {
    let mut entries = Map::new();
    if let Ok(directory) = fs::read_dir("/sys/class/net") {
        for entry in directory.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !include(&name) {
                continue;
            }
            let status = fs::read_to_string(entry.path().join("operstate"))
                .unwrap_or_else(|_| "unknown".into());
            let mut properties = Map::new();
            properties.insert("status".into(), json!(status.trim()));
            if let Some(address) = read_attribute(&entry.path().join("address")) {
                properties.insert("address".into(), json!(address));
            }
            entries.insert(name, Value::Object(properties));
        }
    }
    present_or_empty(entries)
}

fn usb_devices() -> Value {
    let mut entries = Map::new();
    if let Ok(directory) = fs::read_dir("/sys/bus/usb/devices") {
        for entry in directory.flatten() {
            let Some(vendor) = read_attribute(&entry.path().join("idVendor")) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            entries.insert(
                name,
                json!({
                    "status": "connected",
                    "vendor": vendor,
                    "productId": read_attribute(&entry.path().join("idProduct")),
                    "product": read_attribute(&entry.path().join("product")),
                    "manufacturer": read_attribute(&entry.path().join("manufacturer")),
                    "serial": read_attribute(&entry.path().join("serial"))
                }),
            );
        }
    }
    present_or_empty(entries)
}

fn pci_devices() -> Value {
    let mut entries = Map::new();
    if let Ok(directory) = fs::read_dir("/sys/bus/pci/devices") {
        for entry in directory.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            entries.insert(
                name,
                json!({
                    "status": "connected",
                    "vendor": read_attribute(&entry.path().join("vendor")),
                    "device": read_attribute(&entry.path().join("device")),
                    "class": read_attribute(&entry.path().join("class"))
                }),
            );
        }
    }
    present_or_empty(entries)
}

fn display_connectors(include: impl Fn(&str) -> bool) -> Value {
    let mut entries = Map::new();
    if let Ok(directory) = fs::read_dir("/sys/class/drm") {
        for entry in directory.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.contains('-') || !include(&name) {
                continue;
            }
            let status =
                read_attribute(&entry.path().join("status")).unwrap_or_else(|| "unknown".into());
            entries.insert(name, json!({"status": status}));
        }
    }
    present_or_empty(entries)
}

fn named_entries(root: &str, include: impl Fn(&str) -> bool) -> Value {
    let mut entries = Map::new();
    if let Ok(directory) = fs::read_dir(root) {
        for entry in directory.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if include(&name) {
                entries.insert(name, json!({"status": "connected"}));
            }
        }
    }
    present_or_empty(entries)
}

fn temperatures() -> Value {
    let mut entries = Map::new();
    if let Ok(directory) = fs::read_dir("/sys/class/thermal") {
        for entry in directory.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Ok(raw) = fs::read_to_string(entry.path().join("temp"))
                && let Ok(value) = raw.trim().parse::<f64>()
            {
                entries.insert(name, json!({"temp": value / 1000.0}));
            }
        }
    }
    present_or_empty(entries)
}

fn read_attribute(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim_matches(char::from(0)).trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn present_or_empty(entries: Map<String, Value>) -> Value {
    if entries.is_empty() {
        json!({"not present": " "})
    } else {
        Value::Object(entries)
    }
}
