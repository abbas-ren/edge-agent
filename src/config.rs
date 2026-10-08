use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, bail};
use serde::Deserialize;
use url::Url;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    pub farm: FarmConfig,
    pub device: DeviceConfig,
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FarmConfig {
    pub base_url: String,
    #[serde(default = "default_heartbeat_seconds")]
    pub heartbeat_seconds: u64,
    #[serde(default = "default_request_timeout_seconds")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_tls")]
    pub require_tls: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    pub interface: String,
    pub generation: u8,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub device_type: Option<String>,
    #[serde(default = "default_test_timeout_seconds")]
    pub test_timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathsConfig {
    #[serde(default = "default_uid_path")]
    pub uid: PathBuf,
    #[serde(default = "default_timeout_path")]
    pub heartbeat_timeout: PathBuf,
    #[serde(default = "default_current_test_path")]
    pub current_test: PathBuf,
    #[serde(default = "default_test_root")]
    pub test_root: PathBuf,
    #[serde(default = "default_fw_printenv")]
    pub fw_printenv: PathBuf,
    #[serde(default = "default_fw_setenv")]
    pub fw_setenv: PathBuf,
    #[serde(default = "default_reboot")]
    pub reboot: PathBuf,
    #[serde(default = "default_dmesg")]
    pub dmesg: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            uid: default_uid_path(),
            heartbeat_timeout: default_timeout_path(),
            current_test: default_current_test_path(),
            test_root: default_test_root(),
            fw_printenv: default_fw_printenv(),
            fw_setenv: default_fw_setenv(),
            reboot: default_reboot(),
            dmesg: default_dmesg(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    #[serde(default)]
    pub api_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    #[serde(default = "default_level")]
    pub level: String,
    #[serde(default = "default_log_capacity")]
    pub capacity: usize,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_level(),
            capacity: default_log_capacity(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut config = if path.exists() {
            let text = fs::read_to_string(path)
                .with_context(|| format!("read configuration {}", path.display()))?;
            toml::from_str::<Self>(&text)
                .with_context(|| format!("parse configuration {}", path.display()))?
        } else {
            Self::from_legacy(Path::new("/etc/config/login.cfg"))?
        };
        if let Ok(value) = std::env::var("EDGEAGENT_API_TOKEN") {
            config.security.api_token = Some(value);
        }
        config.validate()?;
        Ok(config)
    }

    fn from_legacy(path: &Path) -> anyhow::Result<Self> {
        let text = fs::read_to_string(path).with_context(|| {
            format!(
                "neither edgeagent config nor legacy {} is readable",
                path.display()
            )
        })?;
        let mut server_ip = None;
        let mut http_port = None;
        let mut interface = "eth0".to_owned();
        let mut generation = 0;
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .with_context(|| format!("legacy config line {} must be key=value", index + 1))?;
            match key.trim() {
                "server_ip" => server_ip = Some(value.trim().to_owned()),
                "http_port" => {
                    http_port = Some(value.trim().parse::<u16>().context("invalid http_port")?)
                }
                "ws_port" => {}
                "iface_name" => interface = value.trim().to_owned(),
                "gen" => generation = value.trim().parse::<u8>().context("invalid gen")?,
                key => bail!("unknown legacy configuration key: {key}"),
            }
        }
        let server_ip = server_ip.context("legacy server_ip is required")?;
        let http_port = http_port.context("legacy http_port is required")?;
        Ok(Self {
            server: ServerConfig::default(),
            farm: FarmConfig {
                base_url: format!("http://{server_ip}:{http_port}"),
                heartbeat_seconds: default_heartbeat_seconds(),
                request_timeout_seconds: default_request_timeout_seconds(),
                require_tls: false,
            },
            device: DeviceConfig {
                interface,
                generation,
                name: None,
                device_type: None,
                test_timeout_seconds: default_test_timeout_seconds(),
            },
            paths: PathsConfig::default(),
            security: SecurityConfig::default(),
            logging: LoggingConfig::default(),
        })
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        let url = Url::parse(&self.farm.base_url).context("farm.base_url must be a URL")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            bail!("farm.base_url must use http or https and include a host");
        }
        if self.farm.require_tls && url.scheme() != "https" {
            bail!("farm.require_tls requires an https base_url");
        }
        if !(1..=3600).contains(&self.farm.heartbeat_seconds) {
            bail!("farm.heartbeat_seconds must be 1..=3600");
        }
        if !(1..=300).contains(&self.farm.request_timeout_seconds) {
            bail!("farm.request_timeout_seconds must be 1..=300");
        }
        if !matches!(self.device.generation, 3..=5) {
            bail!("device.generation must be 3, 4, or 5");
        }
        if !(1..=86_400).contains(&self.device.test_timeout_seconds) {
            bail!("device.test_timeout_seconds must be 1..=86400");
        }
        validate_interface(&self.device.interface)?;
        if let Some(token) = &self.security.api_token
            && (!(32..=256).contains(&token.len())
                || !token.bytes().all(|byte| byte.is_ascii_graphic()))
        {
            bail!("security.api_token must contain 32..=256 visible ASCII characters");
        }
        if !(100..=50_000).contains(&self.logging.capacity) {
            bail!("logging.capacity must be 100..=50000");
        }
        if !matches!(
            self.logging.level.as_str(),
            "trace" | "debug" | "info" | "warn" | "error" | "off"
        ) {
            bail!("logging.level is invalid");
        }
        Ok(())
    }

    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.farm.request_timeout_seconds)
    }

    pub fn farm_ipv4(&self) -> Option<Ipv4Addr> {
        Url::parse(&self.farm.base_url)
            .ok()?
            .host_str()?
            .parse()
            .ok()
    }
}

fn validate_interface(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 15
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        bail!("device.interface must be a valid Linux interface name");
    }
    Ok(())
}

fn default_bind() -> String {
    "0.0.0.0:8888".to_owned()
}
fn default_heartbeat_seconds() -> u64 {
    5
}
fn default_request_timeout_seconds() -> u64 {
    15
}
fn default_test_timeout_seconds() -> u64 {
    3600
}
fn default_tls() -> bool {
    true
}
fn default_uid_path() -> PathBuf {
    "/mnt/emmc/UID.txt".into()
}
fn default_timeout_path() -> PathBuf {
    "/mnt/emmc/timeout".into()
}
fn default_current_test_path() -> PathBuf {
    "/usr/src/currentTest.txt".into()
}
fn default_test_root() -> PathBuf {
    "/usr/src/tests".into()
}
fn default_fw_printenv() -> PathBuf {
    "/usr/bin/fw_printenv".into()
}
fn default_fw_setenv() -> PathBuf {
    "/usr/bin/fw_setenv".into()
}
fn default_reboot() -> PathBuf {
    "/sbin/reboot".into()
}
fn default_dmesg() -> PathBuf {
    "/bin/dmesg".into()
}
fn default_level() -> String {
    "info".to_owned()
}
fn default_log_capacity() -> usize {
    5000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_toml() {
        let config: Config = toml::from_str(
            r#"
            [farm]
            base_url = "https://farm.example"
            [device]
            interface = "eth0"
            generation = 4
        "#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn rejects_cleartext_when_tls_is_required() {
        let config: Config = toml::from_str(
            r#"
            [farm]
            base_url = "http://127.0.0.1:3000"
            [device]
            interface = "eth0"
            generation = 4
        "#,
        )
        .unwrap();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("requires an https")
        );
    }
}
