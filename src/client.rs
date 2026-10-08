use std::{sync::Arc, time::Duration};

use futures_util::SinkExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

use crate::{
    inventory,
    models::{DeviceRegistration, Heartbeat},
    state::AppState,
};

pub async fn run(state: Arc<AppState>) {
    let mut backoff = Duration::from_secs(1);
    let mut resumed = false;
    while !state.shutdown.is_cancelled() {
        if state
            .approved_uid
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .is_none()
            && let Err(error) = register(&state).await
        {
            state.warn(
                "edgeagent::client",
                "device registration failed",
                serde_json::json!({"error": error.to_string()}),
            );
        }
        if state
            .approved_uid
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
        {
            if !resumed {
                crate::operations::resume_test_if_present(Arc::clone(&state)).await;
                resumed = true;
            }
            match heartbeat_session(&state).await {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(error) => state.warn("edgeagent::client", "heartbeat connection failed", serde_json::json!({"error": error.to_string(), "retrySeconds": backoff.as_secs()})),
            }
        }
        tokio::select! {
            () = state.shutdown.cancelled() => break,
            () = tokio::time::sleep(backoff) => {},
        }
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

async fn register(state: &AppState) -> anyhow::Result<()> {
    let snapshot = inventory::snapshot(&state.config);
    let ip = inventory::ip_address(&state.config.device.interface)?;
    let payload = DeviceRegistration {
        device_name: state
            .config
            .device
            .name
            .clone()
            .unwrap_or_else(|| "board".into()),
        device_type: state
            .config
            .device
            .device_type
            .clone()
            .unwrap_or_else(|| board_type().into()),
        device_family: format!("Gen{}", state.config.device.generation),
        mac_address: state.identity.mac_address.clone(),
        ip_address: ip.to_string(),
        software_version: build_version(&state.config.paths.fw_printenv)
            .await
            .unwrap_or_else(|_| "unknown".into()),
        timeout: state.config.farm.heartbeat_seconds,
        interfaces: inventory::registration_interfaces(&snapshot),
        nfs_path: current_nfs_path(),
    };
    let url = format!(
        "{}/api/v1/device/",
        state.config.farm.base_url.trim_end_matches('/')
    );
    let response = state.client.post(url).json(&payload).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("FarmController returned {}", response.status());
    }
    state.info(
        "edgeagent::client",
        "registration sent",
        serde_json::json!({"deviceId": state.identity.device_id}),
    );
    Ok(())
}

async fn heartbeat_session(state: &AppState) -> anyhow::Result<()> {
    let mut url = Url::parse(&state.config.farm.base_url)?;
    url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
        .map_err(|_| anyhow::anyhow!("invalid FarmController scheme"))?;
    url.set_path("/ws");
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("deviceId", &state.identity.device_id);
    let (mut socket, _) = connect_async(url.as_str()).await?;
    state.info(
        "edgeagent::client",
        "heartbeat WebSocket connected",
        serde_json::json!({}),
    );
    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => { let _ = socket.close(None).await; return Ok(()); }
            () = state.heartbeat_changed.notified() => continue,
            () = tokio::time::sleep(Duration::from_secs(state.heartbeat_seconds())) => {
                let heartbeat = Heartbeat {
                    message_type: "heartbeat",
                    uid: state.identity.device_id.clone(),
                    ip: inventory::ip_address(&state.config.device.interface)?.to_string(),
                    data: inventory::snapshot(&state.config),
                };
                socket.send(Message::Text(serde_json::to_string(&heartbeat)?.into())).await?;
            }
            message = futures_util::StreamExt::next(&mut socket) => match message {
                Some(Ok(Message::Ping(bytes))) => socket.send(Message::Pong(bytes)).await?,
                Some(Ok(Message::Close(_))) | None => anyhow::bail!("heartbeat WebSocket closed"),
                Some(Err(error)) => return Err(error.into()),
                _ => {},
            }
        }
    }
}

async fn build_version(command: &std::path::Path) -> anyhow::Result<String> {
    let output = tokio::process::Command::new(command)
        .arg("build_ver")
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!("fw_printenv failed");
    }
    let text = String::from_utf8(output.stdout)?;
    Ok(text
        .trim()
        .split_once('=')
        .map(|(_, value)| value)
        .unwrap_or(text.trim())
        .to_owned())
}

fn board_type() -> &'static str {
    let model = std::fs::read_to_string("/proc/device-tree/model").unwrap_or_default();
    if model.contains("White Hawk") {
        "v4h"
    } else if model.contains("Gray Hawk") {
        "v4m"
    } else if model.contains("Ironhide") {
        "x5h"
    } else if model.contains("Salvator-X") {
        "gen3"
    } else {
        "unknown"
    }
}

fn current_nfs_path() -> Option<String> {
    let command_line = std::fs::read_to_string("/proc/cmdline").ok()?;
    let value = command_line
        .split_whitespace()
        .find_map(|part| part.strip_prefix("nfsroot="))?;
    Some(value.split(',').next()?.split_once(':')?.1.to_owned())
}
