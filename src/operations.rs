use std::{
    fs,
    net::Ipv4Addr,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, bail};
use futures_util::SinkExt;
use tokio::{fs::OpenOptions, io::AsyncWriteExt, process::Command};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

use crate::{
    identity::atomic_write,
    models::FlashRequest,
    state::{AppState, TestControl},
};

pub fn validate_flash(request: &FlashRequest) -> anyhow::Result<()> {
    request
        .server_ip
        .parse::<Ipv4Addr>()
        .context("server_ip must be IPv4")?;
    validate_remote_path(&request.nfs, "nfs")?;
    validate_remote_path(&request.image, "image")?;
    validate_remote_path(&request.dtb, "dtb")?;
    if request.build_version.is_empty()
        || request.build_version.len() > 256
        || request.build_version.chars().any(char::is_control)
    {
        bail!("build_ver must be 1..=256 printable characters");
    }
    if let Some(test_id) = &request.test_id {
        validate_component(test_id, "testId")?;
    }
    Ok(())
}

pub async fn configure_boot(state: &AppState, request: &FlashRequest) -> anyhow::Result<()> {
    validate_flash(request)?;
    let (image_address, dtb_address, bootcmd, network_interface) =
        if state.config.device.generation == 5 {
            (
                "0x9e680000",
                "0x9e600000",
                "run IMAGE; run DTB;booti 0x9e680000 - 0x9e600000",
                "tsn5",
            )
        } else {
            (
                "0x48080000",
                "0x48000000",
                "run IMAGE; run DTB;booti 0x48080000 - 0x48000000",
                "eth0",
            )
        };
    let bootargs = if state.config.device.generation == 5 {
        let local_ip = crate::inventory::ip_address(&state.config.device.interface)?;
        format!(
            "rw root=/dev/nfs nfsroot={}:{},nfsvers=3 ip={}:::::{} pd_ignore_unused clk_ignore_unused",
            request.server_ip, request.nfs, local_ip, network_interface
        )
    } else {
        format!(
            "rw root=/dev/nfs nfsroot={}:{},nfsvers=3 ip=dhcp:::::{} cma=560M@0x80000000 clk_ignore_unused",
            request.server_ip, request.nfs, network_interface
        )
    };
    let values = [
        ("IMAGE", format!("tftp {image_address} {}", request.image)),
        ("DTB", format!("tftp {dtb_address} {}", request.dtb)),
        ("serverip", request.server_ip.clone()),
        ("bootargs", bootargs),
        ("build_ver", request.build_version.clone()),
        ("bootcmd", bootcmd.to_owned()),
    ];
    for (key, value) in values {
        run_checked(&state.config.paths.fw_setenv, [key, value.as_str()])
            .await
            .with_context(|| format!("set U-Boot variable {key}"))?;
    }
    state.info(
        "edgeagent::flash",
        "boot environment updated",
        serde_json::json!({"nfs": request.nfs, "buildVersion": request.build_version}),
    );
    Ok(())
}

pub fn current_nfs_path() -> Option<String> {
    let command_line = fs::read_to_string("/proc/cmdline").ok()?;
    let value = command_line
        .split_whitespace()
        .find_map(|part| part.strip_prefix("nfsroot="))?;
    Some(value.split(',').next()?.split_once(':')?.1.to_owned())
}

pub fn schedule_reboot(state: Arc<AppState>) {
    state
        .rebooting
        .store(true, std::sync::atomic::Ordering::Release);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let callback = format!(
            "{}/api/v1/device/{}/flashing",
            state.config.farm.base_url.trim_end_matches('/'),
            state.identity.device_id
        );
        if let Err(error) = state.client.get(callback).send().await {
            state.warn(
                "edgeagent::reboot",
                "flashing callback failed",
                serde_json::json!({"error": error.to_string()}),
            );
        }
        if state.config.device.generation == 5 {
            let callback = format!(
                "{}/api/v1/device/{}/reboot",
                state.config.farm.base_url.trim_end_matches('/'),
                state.identity.device_id
            );
            if let Err(error) = state.client.get(callback).send().await {
                state.warn(
                    "edgeagent::reboot",
                    "Gen5 reboot callback failed",
                    serde_json::json!({"error": error.to_string()}),
                );
            }
        }
        state.info(
            "edgeagent::reboot",
            "reboot requested",
            serde_json::json!({}),
        );
        match Command::new(&state.config.paths.reboot).status().await {
            Ok(status) if status.success() => {}
            Ok(status) => state.warn(
                "edgeagent::reboot",
                "reboot command failed",
                serde_json::json!({"status": status.to_string()}),
            ),
            Err(error) => state.warn(
                "edgeagent::reboot",
                "cannot execute reboot command",
                serde_json::json!({"error": error.to_string()}),
            ),
        }
    });
}

pub async fn start_test(state: Arc<AppState>, request: FlashRequest) -> anyhow::Result<()> {
    let test_id = request.test_id.clone().context("testId is required")?;
    validate_component(&test_id, "testId")?;
    {
        let mut active = state.active_test.lock().await;
        if active.is_some() {
            bail!("another test is already running");
        }
        let control = Arc::new(TestControl::new(test_id.clone()));
        *active = Some(Arc::clone(&control));
        atomic_write(&state.config.paths.current_test, &test_id)?;
        tokio::spawn(run_test(Arc::clone(&state), request, control));
    }
    Ok(())
}

pub async fn resume_test_if_present(state: Arc<AppState>) {
    let Ok(test_id) = fs::read_to_string(&state.config.paths.current_test) else {
        return;
    };
    let test_id = test_id.trim().to_owned();
    if validate_component(&test_id, "testId").is_err() {
        return;
    }
    let request = FlashRequest {
        nfs: current_nfs_path().unwrap_or_default(),
        image: String::new(),
        dtb: String::new(),
        server_ip: "127.0.0.1".into(),
        test_id: Some(test_id),
        build_version: "resume".into(),
    };
    if let Err(error) = start_test(state.clone(), request).await {
        state.warn(
            "edgeagent::test",
            "cannot resume persisted test",
            serde_json::json!({"error": error.to_string()}),
        );
    }
}

async fn run_test(state: Arc<AppState>, request: FlashRequest, control: Arc<TestControl>) {
    let test_id = control.test_id.clone();
    let result = run_test_inner(&state, &test_id, &control).await;
    if let Err(error) = &result {
        state.warn(
            "edgeagent::test",
            "test execution failed",
            serde_json::json!({"testId": test_id, "error": error.to_string()}),
        );
    }
    let callback = format!(
        "{}/api/v1/device/{}/test-completed",
        state.config.farm.base_url.trim_end_matches('/'),
        state.identity.device_id
    );
    if let Err(error) = state
        .client
        .get(callback)
        .query(&[("testId", &test_id)])
        .send()
        .await
    {
        state.warn(
            "edgeagent::test",
            "test completion callback failed",
            serde_json::json!({"testId": test_id, "error": error.to_string()}),
        );
    }
    if result.is_ok() || control.is_cancelled() {
        let _ = fs::remove_file(&state.config.paths.current_test);
    }
    let mut active = state.active_test.lock().await;
    if active
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, &control))
    {
        *active = None;
    }
    let _ = request;
}

async fn run_test_inner(
    state: &AppState,
    test_id: &str,
    control: &TestControl,
) -> anyhow::Result<()> {
    let list_url = format!(
        "{}/api/v1/device/test/execution/list/{}",
        state.config.farm.base_url.trim_end_matches('/'),
        test_id
    );
    let response = state.client.get(list_url).send().await?;
    if !response.status().is_success() {
        bail!(
            "FarmController testcase list returned {}",
            response.status()
        );
    }
    let case_ids = response.json::<Vec<String>>().await?;
    let case_ids = case_ids
        .into_iter()
        .map(|value| {
            value
                .parse::<i64>()
                .context("testcase ID must be a positive integer")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if case_ids.iter().any(|value| *value <= 0) {
        bail!("testcase ID must be positive");
    }
    let mut url = Url::parse(&state.config.farm.base_url)?;
    url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
        .map_err(|_| anyhow::anyhow!("invalid FarmController scheme"))?;
    url.set_path("/ws");
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("deviceId", &state.identity.device_id)
        .append_pair("testId", test_id);
    let (mut socket, _) = connect_async(url.as_str()).await?;
    for case_id in case_ids {
        if control.is_cancelled() {
            bail!("test execution cancelled");
        }
        run_case(state, test_id, case_id, control).await?;
        socket
            .send(Message::Text(case_id.to_string().into()))
            .await?;
    }
    socket.close(None).await?;
    state.info(
        "edgeagent::test",
        "test execution completed",
        serde_json::json!({"testId": test_id}),
    );
    Ok(())
}

async fn run_case(
    state: &AppState,
    test_id: &str,
    case_id: i64,
    control: &TestControl,
) -> anyhow::Result<()> {
    let directory = contained_directory(&state.config.paths.test_root, test_id)?;
    let script = directory.join(format!("{case_id}.sh"));
    let canonical_script = fs::canonicalize(&script)
        .with_context(|| format!("resolve test script {}", script.display()))?;
    if !canonical_script.starts_with(&directory) || !canonical_script.is_file() {
        bail!("test script escapes execution directory");
    }
    let output_path = directory.join(format!("{case_id}.output"));
    let output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&output_path)
        .await?;
    let error_output = output.try_clone().await?;
    let mut child = Command::new(&canonical_script)
        .current_dir(&directory)
        .stdin(Stdio::null())
        .stdout(output.into_std().await)
        .stderr(error_output.into_std().await)
        .kill_on_drop(true)
        .spawn()?;
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(state.config.device.test_timeout_seconds);
    loop {
        if control.is_cancelled() {
            child.kill().await?;
            bail!("test execution cancelled");
        }
        if let Some(status) = child.try_wait()? {
            let mut output = OpenOptions::new().append(true).open(&output_path).await?;
            output
                .write_all(
                    format!(
                        "\nEDGEAGENT_EXIT_CODE={}\n{}\n",
                        status.code().unwrap_or(-1),
                        if status.success() { "PASS" } else { "FAIL" }
                    )
                    .as_bytes(),
                )
                .await?;
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            child.kill().await?;
            bail!("testcase {case_id} timed out");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let dmesg = Command::new(&state.config.paths.dmesg).output().await;
    if let Ok(output) = dmesg {
        tokio::fs::write(directory.join(format!("{case_id}.txt")), output.stdout).await?;
    }
    state.info(
        "edgeagent::test",
        "testcase completed",
        serde_json::json!({"testId": test_id, "caseId": case_id}),
    );
    Ok(())
}

pub async fn cancel_test(state: &AppState, test_id: &str) -> anyhow::Result<bool> {
    validate_component(test_id, "testId")?;
    let active = state.active_test.lock().await;
    if let Some(control) = active.as_ref().filter(|control| control.test_id == test_id) {
        control.cancel();
        return Ok(true);
    }
    Ok(false)
}

fn contained_directory(root: &Path, test_id: &str) -> anyhow::Result<PathBuf> {
    validate_component(test_id, "testId")?;
    let root = fs::canonicalize(root)?;
    let directory = fs::canonicalize(root.join(test_id))?;
    if !directory.starts_with(&root) || !directory.is_dir() {
        bail!("test directory escapes configured root");
    }
    Ok(directory)
}

fn validate_component(value: &str, field: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 128
        || Path::new(value).components().count() != 1
        || !matches!(
            Path::new(value).components().next(),
            Some(Component::Normal(_))
        )
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        bail!("{field} must be a safe path component");
    }
    Ok(())
}

fn validate_remote_path(value: &str, field: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 1024
        || !value.starts_with('/')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.'))
        || value.split('/').any(|part| part == "..")
    {
        bail!("{field} must be an absolute safe path");
    }
    Ok(())
}

async fn run_checked<'a>(
    program: &Path,
    arguments: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<()> {
    let status = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .await?;
    if !status.success() {
        bail!("{} exited with {status}", program.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_remote_input_without_shell_interpretation() {
        let request = FlashRequest {
            nfs: "/nfs/device/build".into(),
            image: "/device/build/Image".into(),
            dtb: "/device/build/board.dtb".into(),
            server_ip: "192.0.2.10".into(),
            test_id: Some("test-1".into()),
            build_version: "v1.2.3".into(),
        };
        validate_flash(&request).unwrap();
        let mut injected = request;
        injected.image = "/build/Image;reboot".into();
        assert!(validate_flash(&injected).is_err());
    }
}
