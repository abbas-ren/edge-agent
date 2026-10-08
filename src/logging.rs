use std::{
    collections::VecDeque,
    sync::{Arc, RwLock},
};

use anyhow::bail;
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub target: String,
    pub message: String,
    pub fields: serde_json::Value,
}

#[derive(Debug)]
struct LogState {
    level: String,
    next_sequence: u64,
    entries: VecDeque<LogEntry>,
}

#[derive(Clone, Debug)]
pub struct LogBuffer {
    capacity: usize,
    state: Arc<RwLock<LogState>>,
}

impl LogBuffer {
    pub fn new(capacity: usize, level: String) -> Self {
        Self {
            capacity,
            state: Arc::new(RwLock::new(LogState {
                level,
                next_sequence: 1,
                entries: VecDeque::with_capacity(capacity),
            })),
        }
    }

    pub fn push(&self, level: &str, target: &str, message: &str, fields: serde_json::Value) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if !enabled(&state.level, level) {
            return;
        }
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        if state.entries.len() == self.capacity {
            state.entries.pop_front();
        }
        state.entries.push_back(LogEntry {
            sequence,
            timestamp: Utc::now(),
            level: level.to_owned(),
            target: target.to_owned(),
            message: message.to_owned(),
            fields,
        });
    }

    pub fn recent(&self, limit: usize) -> Vec<LogEntry> {
        let state = self.state.read().unwrap_or_else(|error| error.into_inner());
        state
            .entries
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    pub fn level(&self) -> String {
        self.state
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .level
            .clone()
    }

    pub fn set_level(&self, level: &str) -> anyhow::Result<String> {
        if !matches!(level, "trace" | "debug" | "info" | "warn" | "error" | "off") {
            bail!("unsupported log level");
        }
        self.state
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .level = level.to_owned();
        Ok(level.to_owned())
    }
}

fn enabled(configured: &str, event: &str) -> bool {
    let rank = |value| match value {
        "trace" => 0,
        "debug" => 1,
        "info" => 2,
        "warn" => 3,
        "error" => 4,
        _ => 5,
    };
    rank(event) >= rank(configured) && configured != "off"
}

pub fn init_console() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "edgeagent_rs=info".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_is_bounded_and_filtered() {
        let logs = LogBuffer::new(2, "info".into());
        logs.push("debug", "test", "hidden", serde_json::json!({}));
        for message in ["one", "two", "three"] {
            logs.push("info", "test", message, serde_json::json!({}));
        }
        assert_eq!(
            logs.recent(10)
                .iter()
                .map(|entry| entry.message.as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
    }
}
