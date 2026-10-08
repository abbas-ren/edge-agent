use std::{fs, io::Write, path::Path};

use anyhow::{Context, bail};

use crate::config::Config;

#[derive(Clone, Debug)]
pub struct Identity {
    pub mac_address: String,
    pub device_id: String,
    pub approved_uid: Option<String>,
}

impl Identity {
    pub fn load(config: &Config) -> anyhow::Result<Self> {
        let mac_path = Path::new("/sys/class/net")
            .join(&config.device.interface)
            .join("address");
        let mac_address = fs::read_to_string(&mac_path)
            .with_context(|| format!("read MAC address from {}", mac_path.display()))?;
        let mac_address = normalize_mac(&mac_address)?;
        let device_id = mac_address.replace(':', "");
        let approved_uid = read_optional_trimmed(&config.paths.uid)?;
        if let Some(uid) = &approved_uid {
            validate_uid(uid, &device_id)?;
        }
        Ok(Self {
            mac_address,
            device_id,
            approved_uid,
        })
    }
}

pub fn normalize_mac(value: &str) -> anyhow::Result<String> {
    let compact = value
        .trim()
        .chars()
        .filter(|character| !matches!(character, ':' | '-'))
        .collect::<String>()
        .to_ascii_lowercase();
    if compact.len() != 12 || !compact.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid MAC address");
    }
    Ok(compact
        .as_bytes()
        .chunks(2)
        .map(|chunk| std::str::from_utf8(chunk).expect("ASCII"))
        .collect::<Vec<_>>()
        .join(":"))
}

pub fn validate_uid(uid: &str, device_id: &str) -> anyhow::Result<String> {
    let normalized = uid.trim().to_ascii_lowercase();
    if normalized != device_id {
        bail!("approved UID does not match this device MAC");
    }
    Ok(normalized)
}

pub fn atomic_write(path: &Path, value: &str) -> anyhow::Result<()> {
    let parent = path.parent().context("persistent path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state"),
        std::process::id()
    ));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(value.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn read_optional_trimmed(path: &Path) -> anyhow::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value.trim().to_owned())),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_mac_identity() {
        assert_eq!(
            normalize_mac("AA-BB-CC-DD-EE-FF\n").unwrap(),
            "aa:bb:cc:dd:ee:ff"
        );
        assert_eq!(
            validate_uid("aabbccddeeff", "aabbccddeeff").unwrap(),
            "aabbccddeeff"
        );
    }
}
