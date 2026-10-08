use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegistration {
    pub device_name: String,
    pub device_type: String,
    pub device_family: String,
    pub mac_address: String,
    pub ip_address: String,
    pub software_version: String,
    pub timeout: u64,
    pub interfaces: BTreeMap<String, Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nfs_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Heartbeat {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub uid: String,
    pub ip: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ApprovalRequest {
    #[serde(rename = "UID")]
    pub uid: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    #[serde(rename = "UID")]
    pub uid: String,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatConfigRequest {
    #[serde(alias = "value")]
    pub timeout: u64,
}

#[derive(Debug, Deserialize)]
pub struct LogLevelRequest {
    pub level: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FlashRequest {
    pub nfs: String,
    pub image: String,
    pub dtb: String,
    pub server_ip: String,
    #[serde(rename = "testId")]
    pub test_id: Option<String>,
    #[serde(rename = "build_ver")]
    pub build_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequest {
    pub test_id: String,
}
