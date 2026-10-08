use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Deserialize;

use crate::{
    identity::{atomic_write, validate_uid},
    models::{
        ApprovalRequest, CancelRequest, DeleteRequest, FlashRequest, HeartbeatConfigRequest,
        LogLevelRequest,
    },
    operations,
    state::AppState,
};

#[derive(Debug, Deserialize)]
struct LogQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdminControlPatch {
    log_level: Option<String>,
    heartbeat_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AdminControlAction {
    Restart,
    CancelActiveTest,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdminControlActionRequest {
    action: AdminControlAction,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/status", get(status))
        .route("/logs", get(logs))
        .route("/logs/level", put(log_level))
        .route(
            "/admin/control",
            get(admin_control_get)
                .patch(admin_control_patch)
                .post(admin_control_action),
        )
        .route("/approve", post(approve))
        .route("/delete", post(delete))
        .route("/configure/heartbeat", post(configure_heartbeat))
        .route("/cancel", post(cancel))
        .route("/flash", post(flash))
        .route("/test-execution", post(test_execution))
        .with_state(state)
}

async fn admin_control_get(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    admin_control_snapshot(&state).await.into_response()
}

async fn admin_control_patch(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<AdminControlPatch>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    if request.log_level.is_none() && request.heartbeat_seconds.is_none() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "empty_patch",
            "at least one control value is required",
        );
    }
    if let Some(level) = request.log_level
        && let Err(error) = state.logs.set_level(&level)
    {
        return api_error(StatusCode::BAD_REQUEST, "invalid_level", error.to_string());
    }
    if let Some(seconds) = request.heartbeat_seconds {
        if let Err(message) = validate_heartbeat_seconds(seconds) {
            return api_error(StatusCode::BAD_REQUEST, "invalid_heartbeat", message);
        }
        if let Err(error) =
            atomic_write(&state.config.paths.heartbeat_timeout, &seconds.to_string())
        {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "persistence",
                error.to_string(),
            );
        }
        state.set_heartbeat_seconds(seconds);
    }
    state.info(
        "edgeagent::api",
        "administrator updated live controls",
        serde_json::json!({"peer": peer.ip()}),
    );
    admin_control_snapshot(&state).await.into_response()
}

async fn admin_control_action(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<AdminControlActionRequest>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    match request.action {
        AdminControlAction::Restart => {
            schedule_restart();
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "accepted": true,
                    "action": "restart",
                    "service": "edgeagent-rs.service"
                })),
            )
                .into_response()
        }
        AdminControlAction::CancelActiveTest => {
            let test_id = state
                .active_test
                .lock()
                .await
                .as_ref()
                .map(|test| test.test_id.clone());
            let Some(test_id) = test_id else {
                return api_error(
                    StatusCode::CONFLICT,
                    "no_active_test",
                    "there is no active test to cancel",
                );
            };
            match operations::cancel_test(&state, &test_id).await {
                Ok(true) => Json(serde_json::json!({
                    "accepted": true,
                    "action": "cancelActiveTest",
                    "testId": test_id
                }))
                .into_response(),
                Ok(false) => api_error(
                    StatusCode::CONFLICT,
                    "no_active_test",
                    "the active test already finished",
                ),
                Err(error) => {
                    api_error(StatusCode::BAD_REQUEST, "cancel_failed", error.to_string())
                }
            }
        }
    }
}

async fn admin_control_snapshot(state: &AppState) -> Json<serde_json::Value> {
    let active_test = state
        .active_test
        .lock()
        .await
        .as_ref()
        .map(|test| test.test_id.clone());
    Json(serde_json::json!({
        "service": "edgeagent",
        "version": env!("CARGO_PKG_VERSION"),
        "restartPending": false,
        "configuration": {
            "logLevel": state.logs.level(),
            "heartbeatSeconds": state.heartbeat_seconds()
        },
        "runtime": {
            "deviceId": state.identity.device_id,
            "generation": state.config.device.generation,
            "approved": state.approved_uid.read().unwrap_or_else(|error| error.into_inner()).is_some(),
            "activeTest": active_test,
            "rebooting": state.rebooting.load(std::sync::atomic::Ordering::Acquire)
        },
        "capabilities": ["logging", "heartbeat", "restart", "cancelActiveTest"],
        "constraints": {
            "heartbeatSeconds": {"min": 1, "max": 3600},
            "logLevels": ["trace", "debug", "info", "warn", "error", "off"],
            "restartService": "edgeagent-rs.service"
        }
    }))
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    if state.rebooting.load(std::sync::atomic::Ordering::Acquire) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Re-booting"})),
        )
            .into_response();
    }
    Json(serde_json::json!({"status": "ok"})).into_response()
}

async fn ready(State(state): State<Arc<AppState>>) -> Response {
    let approved = state
        .approved_uid
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .is_some();
    let status = if approved {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(serde_json::json!({"ready": approved, "approved": approved})),
    )
        .into_response()
}

async fn status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "service": "edgeagent-rs",
        "version": env!("CARGO_PKG_VERSION"),
        "deviceId": state.identity.device_id,
        "generation": state.config.device.generation,
        "approved": state.approved_uid.read().unwrap_or_else(|error| error.into_inner()).is_some(),
        "activeTest": state.active_test.lock().await.as_ref().map(|test| test.test_id.clone())
    }))
}

async fn logs(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<LogQuery>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    Json(serde_json::json!({"level": state.logs.level(), "logs": state.logs.recent(query.limit.unwrap_or(500).clamp(1, 5000))})).into_response()
}

async fn log_level(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<LogLevelRequest>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    match state.logs.set_level(&request.level) {
        Ok(level) => Json(serde_json::json!({"level": level})).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, "invalid_level", error.to_string()),
    }
}

async fn approve(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<ApprovalRequest>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    let uid = match validate_uid(&request.uid, &state.identity.device_id) {
        Ok(uid) => uid,
        Err(error) => {
            return api_error(StatusCode::CONFLICT, "identity_mismatch", error.to_string());
        }
    };
    if let Err(error) = atomic_write(&state.config.paths.uid, &uid) {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "persistence",
            error.to_string(),
        );
    }
    *state
        .approved_uid
        .write()
        .unwrap_or_else(|error| error.into_inner()) = Some(uid);
    state.info(
        "edgeagent::api",
        "device approved",
        serde_json::json!({"peer": peer.ip()}),
    );
    Json(serde_json::json!({"OK": true})).into_response()
}

async fn delete(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<DeleteRequest>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    if validate_uid(&request.uid, &state.identity.device_id).is_err() {
        return api_error(StatusCode::NOT_FOUND, "uid_not_found", "UID not found");
    }
    for path in [
        &state.config.paths.uid,
        &state.config.paths.heartbeat_timeout,
    ] {
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "persistence",
                error.to_string(),
            );
        }
    }
    *state
        .approved_uid
        .write()
        .unwrap_or_else(|error| error.into_inner()) = None;
    state.info(
        "edgeagent::api",
        "device approval removed",
        serde_json::json!({"peer": peer.ip()}),
    );
    Json(serde_json::json!({"OK": true})).into_response()
}

async fn configure_heartbeat(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<HeartbeatConfigRequest>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    if let Err(message) = validate_heartbeat_seconds(request.timeout) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_timeout", message);
    }
    if let Err(error) = atomic_write(
        &state.config.paths.heartbeat_timeout,
        &request.timeout.to_string(),
    ) {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "persistence",
            error.to_string(),
        );
    }
    state.set_heartbeat_seconds(request.timeout);
    Json(serde_json::json!({"OK": true, "timeout": request.timeout})).into_response()
}

fn validate_heartbeat_seconds(seconds: u64) -> Result<(), &'static str> {
    if (1..=3600).contains(&seconds) {
        Ok(())
    } else {
        Err("heartbeatSeconds must be 1..=3600 seconds")
    }
}

fn schedule_restart() {
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        let process_id = std::process::id().to_string();
        match tokio::process::Command::new("kill")
            .args(["-TERM", &process_id])
            .status()
            .await
        {
            Ok(status) if status.success() => tracing::info!("agent restart requested"),
            Ok(status) => tracing::error!(%status, "agent restart signal was rejected"),
            Err(error) => tracing::error!(%error, "cannot signal agent restart"),
        }
    });
}

async fn cancel(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<CancelRequest>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    match operations::cancel_test(&state, &request.test_id).await {
        Ok(true) => Json(serde_json::json!({"OK": true, "cancelled": true})).into_response(),
        Ok(false) => api_error(
            StatusCode::NOT_FOUND,
            "test_not_found",
            "active test was not found",
        ),
        Err(error) => api_error(StatusCode::BAD_REQUEST, "invalid_test", error.to_string()),
    }
}

async fn flash(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<FlashRequest>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    if let Err(error) = operations::validate_flash(&request) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "flash_configuration",
            error.to_string(),
        );
    }
    if operations::current_nfs_path().as_deref() == Some(request.nfs.as_str()) {
        return Json(serde_json::json!({"OK": true, "rebooting": false})).into_response();
    }
    if let Err(error) = operations::configure_boot(&state, &request).await {
        return api_error(
            StatusCode::BAD_REQUEST,
            "flash_configuration",
            error.to_string(),
        );
    }
    operations::schedule_reboot(Arc::clone(&state));
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"OK": true, "rebooting": true})),
    )
        .into_response()
}

async fn test_execution(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<FlashRequest>,
) -> Response {
    if !authorize(&state, peer.ip(), &headers) {
        return unauthorized();
    }
    if request.test_id.is_none() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "test_configuration",
            "testId is required",
        );
    }
    if let Err(error) = operations::validate_flash(&request) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "test_configuration",
            error.to_string(),
        );
    }
    let same_root = operations::current_nfs_path().as_deref() == Some(request.nfs.as_str());
    if !same_root {
        if let Err(error) = operations::configure_boot(&state, &request).await {
            return api_error(
                StatusCode::BAD_REQUEST,
                "test_configuration",
                error.to_string(),
            );
        }
        if let Some(test_id) = request.test_id.as_deref()
            && let Err(error) = atomic_write(&state.config.paths.current_test, test_id)
        {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "persistence",
                error.to_string(),
            );
        }
        operations::schedule_reboot(Arc::clone(&state));
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"OK": true, "rebooting": true})),
        )
            .into_response();
    }
    match operations::start_test(Arc::clone(&state), request).await {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"OK": true, "started": true})),
        )
            .into_response(),
        Err(error) => api_error(StatusCode::CONFLICT, "test_start", error.to_string()),
    }
}

fn authorize(state: &AppState, peer: IpAddr, headers: &HeaderMap) -> bool {
    if let Some(token) = state.config.security.api_token.as_deref() {
        let supplied = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        if supplied == Some(token) {
            return true;
        }
    } else if state
        .config
        .farm_ipv4()
        .is_some_and(|expected| peer == IpAddr::V4(expected))
        || peer.is_loopback()
    {
        return true;
    }
    false
}

fn unauthorized() -> Response {
    api_error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "request is not authorized",
    )
}

fn api_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message.into()}})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::validate_heartbeat_seconds;

    #[test]
    fn heartbeat_control_is_bounded() {
        assert!(validate_heartbeat_seconds(1).is_ok());
        assert!(validate_heartbeat_seconds(3600).is_ok());
        assert!(validate_heartbeat_seconds(0).is_err());
        assert!(validate_heartbeat_seconds(3601).is_err());
    }
}
