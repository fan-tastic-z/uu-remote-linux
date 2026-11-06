//! The 我的设备 card's host side: polls the account's device list in-process (the viewer
//! tree's API client, without its SDL/GTK3 parts — those live in the `uuremote-viewer`
//! binary) and runs one detached viewer child per remote session, mirroring the launcher
//! window's verified behavior (`viewer::ui`): `connect <id> false - echo legacy`, the
//! device name in `UU_VIEWER_DEVICE_NAME`, and the child's last non-log stderr line as
//! the failure reason. Device names and ids reach the window only, never a log.

use super::state::{self, DeviceRow, Patch};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::BufRead;
use std::sync::{LazyLock, Mutex};

/// A session action requested by the window (engine thread), served by the host task.
enum Request {
    Connect { id: String, name: String, watch: bool },
    Disconnect { id: String },
}

static REQUESTS: Mutex<VecDeque<Request>> = Mutex::new(VecDeque::new());
/// The live session children's pids (the owning watcher threads hold the `Child` handles).
/// HashMap/HashSet have no const constructor; LazyLock is the const-context way.
static SESSIONS: LazyLock<Mutex<HashMap<String, i32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Devices the user asked to disconnect: their watcher reports no failure line.
static KILLED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

pub(crate) fn request_connect(id: &str, name: &str, watch: bool) {
    let request = Request::Connect { id: id.to_owned(), name: name.to_owned(), watch };
    if let Ok(mut queue) = REQUESTS.lock() { if queue.len() < 16 { queue.push_back(request); } }
}

pub(crate) fn request_disconnect(id: &str) {
    let request = Request::Disconnect { id: id.to_owned() };
    if let Ok(mut queue) = REQUESTS.lock() { if queue.len() < 16 { queue.push_back(request); } }
}

/// Starts the poll/serve task once, from the host's runtime (dies with it; session
/// children are detached on purpose — they survive a host restart, like the launcher's).
pub(crate) fn start() {
    tokio::spawn(async move {
        let mut next_refresh = tokio::time::Instant::now();
        // The last failure line per device, kept so a list refresh does not clear it.
        let mut errors: HashMap<String, String> = HashMap::new();
        loop {
            loop {
                let request = REQUESTS.lock().ok().and_then(|mut queue| queue.pop_front());
                let Some(request) = request else { break };
                match request {
                    Request::Connect { id, name, watch } => {
                        errors.remove(&id);
                        spawn_session(&id, &name, watch);
                    }
                    Request::Disconnect { id } => disconnect(&id),
                }
            }
            if tokio::time::Instant::now() >= next_refresh {
                next_refresh = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
                refresh(&mut errors).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
}

/// Fetches the account's device list and pushes it to the window (signed-out: nothing).
async fn refresh(errors: &mut HashMap<String, String>) {
    let Ok(Some(file)) = crate::config::read_fresh() else { return };
    let Ok(credentials) = crate::auth::Credentials::load(Some(&file)) else { return };
    let api = crate::viewer::api::Api::new(crate::auth::UUAuthClient::new(credentials), crate::auth::BASE_URL);
    let data = match api.device_list().await {
        Ok(data) => data,
        Err(crate::viewer::api::Error::AuthenticationExpired) => return, // the login flow owns re-sign-in
        Err(error) => { log::info!("Device list refresh failed: {error:?}"); return }
    };
    let empty = Vec::new();
    let list = data.get("my_binded_devices").and_then(serde_json::Value::as_array).unwrap_or(&empty);
    let current = data.pointer("/current_device/device_id").and_then(serde_json::Value::as_str);
    let running: HashSet<String> = SESSIONS.lock().map(|sessions| sessions.keys().cloned().collect()).unwrap_or_default();
    let rows = list.iter().filter_map(|device| {
        let id = device.get("device_id")?.as_str()?.to_owned();
        let name = device.get("alias").and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty()).unwrap_or(&id).to_owned();
        Some(DeviceRow {
            name,
            online: device.get("status").and_then(serde_json::Value::as_str) == Some("CONNECTED"),
            current: current == Some(id.as_str()),
            thumb: thumb_uri(&id),
            running: running.contains(&id),
            error: errors.get(&id).cloned().unwrap_or_default(),
            id,
        })
    }).collect::<Vec<_>>();
    log::info!("Device list refreshed: n={}", rows.len());
    state::apply(Patch::Devices(rows));
}

/// The cached launcher preview as a data URI, if the viewer ever saved one.
fn thumb_uri(id: &str) -> Option<String> {
    use base64::Engine;
    let bytes = std::fs::read(crate::viewer::thumbs::existing(id)?).ok()?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Kills the session for `id` (SIGTERM, then the watcher's `wait` reaps it).
fn disconnect(id: &str) {
    let Some(pid) = SESSIONS.lock().ok().and_then(|sessions| sessions.get(id).copied()) else { return };
    if let Ok(mut killed) = KILLED.lock() { killed.insert(id.to_owned()); }
    unsafe { libc::kill(pid, libc::SIGTERM); }
}

/// Replaces any live session for the device with a fresh viewer child.
fn spawn_session(id: &str, name: &str, watch: bool) {
    let Some(viewer) = std::env::current_exe().ok()
        .map(|exe| exe.with_file_name("uuremote-viewer"))
        .filter(|path| path.is_file())
    else {
        state::apply(Patch::Session {
            id: id.to_owned(), running: Some(false),
            error: Some(crate::i18n::s("未安装控制端 uuremote-viewer（连接不可用）", "The uuremote-viewer controller is not installed (connect unavailable)").to_owned()),
        });
        return;
    };
    disconnect(id); // replace: the old child's watcher reaps it
    let mut command = std::process::Command::new(&viewer);
    command.args(["connect", id, "false", "-", "echo", "legacy"]);
    if watch { command.arg("--view-only"); }
    command.env("UU_VIEWER_DEVICE_NAME", name)
        .env("UU_VIEWER_COMMANDS", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            log::warn!("Viewer session spawn failed: {error}");
            state::apply(Patch::Session {
                id: id.to_owned(), running: Some(false),
                error: Some(crate::i18n::s("无法启动会话进程", "Cannot start the session process").to_owned()),
            });
            return;
        }
    };
    let pid = child.id() as i32;
    if let Ok(mut sessions) = SESSIONS.lock() { sessions.insert(id.to_owned(), pid); }
    state::apply(Patch::Session { id: id.to_owned(), running: Some(true), error: Some(String::new()) });
    let stderr = child.stderr.take();
    let watched = id.to_owned();
    // The watcher owns the Child: stderr to EOF gives the last failure line (log lines
    // start with '['), then wait() reaps and clears the session state.
    std::thread::spawn(move || {
        let mut last = String::new();
        if let Some(stderr) = stderr {
            for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.starts_with('[') && !line.trim().is_empty() { last = line.chars().take(160).collect(); }
            }
        }
        let ok = child.wait().map(|status| status.success()).unwrap_or(false);
        if let Ok(mut sessions) = SESSIONS.lock() { sessions.remove(&watched); }
        let killed = KILLED.lock().map(|mut killed| killed.remove(&watched)).unwrap_or(false);
        let error = if killed || ok { String::new() } else { failure_reason(&last) };
        state::apply(Patch::Session { id: watched, running: Some(false), error: Some(error) });
    });
}

/// The viewer's known failure lines, as the launcher window maps them.
fn failure_reason(last: &str) -> String {
    match last {
        "" => crate::i18n::s("连接已断开", "Disconnected").to_owned(),
        "CONTROL echo timed out" => crate::i18n::s(
            "连接超时：被控端没有响应（官方主机互通尚未完成）",
            "Timed out: the remote device did not answer (official host interop is not finished)").to_owned(),
        "Remote session released" => crate::i18n::s("被控端结束了本次连接", "The remote device ended the connection").to_owned(),
        other => other.to_owned(),
    }
}
