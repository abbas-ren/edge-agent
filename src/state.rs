use std::{
    fs,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use anyhow::Context;
use tokio::sync::{Mutex, Notify};

use crate::{config::Config, identity::Identity, logging::LogBuffer};

#[derive(Debug, Default)]
pub struct Shutdown {
    stopped: std::sync::atomic::AtomicBool,
    notify: Notify,
}

impl Shutdown {
    pub fn cancel(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::Acquire)
    }
    pub async fn cancelled(&self) {
        if !self.is_cancelled() {
            self.notify.notified().await;
        }
    }
}

#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub identity: Identity,
    pub logs: LogBuffer,
    pub approved_uid: RwLock<Option<String>>,
    pub active_test: Mutex<Option<Arc<TestControl>>>,
    pub heartbeat_seconds: AtomicU64,
    pub heartbeat_changed: Notify,
    pub rebooting: AtomicBool,
    pub shutdown: Arc<Shutdown>,
    pub client: reqwest::Client,
}

#[derive(Debug)]
pub struct TestControl {
    pub test_id: String,
    cancelled: AtomicBool,
}

impl TestControl {
    pub fn new(test_id: String) -> Self {
        Self {
            test_id,
            cancelled: AtomicBool::new(false),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl AppState {
    pub fn new(config: Config, identity: Identity) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout())
            .build()
            .context("build HTTP client")?;
        let heartbeat_seconds = fs::read_to_string(&config.paths.heartbeat_timeout)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| (1..=3600).contains(value))
            .unwrap_or(config.farm.heartbeat_seconds);
        Ok(Self {
            logs: LogBuffer::new(config.logging.capacity, config.logging.level.clone()),
            approved_uid: RwLock::new(identity.approved_uid.clone()),
            config,
            identity,
            active_test: Mutex::new(None),
            heartbeat_seconds: AtomicU64::new(heartbeat_seconds),
            heartbeat_changed: Notify::new(),
            rebooting: AtomicBool::new(false),
            shutdown: Arc::new(Shutdown::default()),
            client,
        })
    }

    pub fn log(&self, level: &str, target: &str, message: &str, fields: serde_json::Value) {
        self.logs.push(level, target, message, fields.clone());
        match level {
            "error" => tracing::error!(target, message, fields = %fields),
            "warn" => tracing::warn!(target, message, fields = %fields),
            "debug" | "trace" => tracing::debug!(target, message, fields = %fields),
            _ => tracing::info!(target, message, fields = %fields),
        }
    }
    pub fn info(&self, target: &str, message: &str, fields: serde_json::Value) {
        self.log("info", target, message, fields);
    }
    pub fn warn(&self, target: &str, message: &str, fields: serde_json::Value) {
        self.log("warn", target, message, fields);
    }
    pub fn heartbeat_seconds(&self) -> u64 {
        self.heartbeat_seconds.load(Ordering::Acquire)
    }
    pub fn set_heartbeat_seconds(&self, value: u64) {
        self.heartbeat_seconds.store(value, Ordering::Release);
        self.heartbeat_changed.notify_one();
    }
}
