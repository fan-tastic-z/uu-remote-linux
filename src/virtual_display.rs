//! 虚拟显示器 / virtual displays: the viewer's display menu can create extra screens when the
//! host has none free (single monitor, or it is already occupied). The official Windows host
//! plugs in IDD driver devices; the Hyprland equivalent is a headless output
//! (`hyprctl output create headless <name>`), captured through the same multi-screen path
//! (`wayland::select` → the engine binds the named output).
//! Wire shapes recovered from the official GameViewerServer.exe 4.45.1 descriptor (gv_pb):
//! RpcRequest case 12 CreateVirtualDisplayRequest {1: repeated ScreenResolution {1 width,
//! 2 height}, 2 virtual_display_count} → RpcResponse case 8 {1 error_code}; case 13
//! RemoveVirtualDisplayRequest {1 screen_id} → case 9 {1 error_code}. The response carries no
//! screen list: the viewer learns the new screens from the re-sent Message 7 ScreenSources.

use crate::protocol::{decode_wire, WireField, WireValue};

/// Official RpcRequest cases handled here, and their response cases.
pub const CREATE_REQUEST: u32 = 12;
pub const REMOVE_REQUEST: u32 = 13;
const CREATE_RESPONSE: u64 = 8;
const REMOVE_RESPONSE: u64 = 9;
/// error_code 0 = success is this codebase's protobuf convention (CaptureSettingResponse,
/// file transfer results); the official failure values are not yet observed live.
const ERROR_OK: u64 = 0;
const ERROR_FAILED: u64 = 1;
/// Sanity cap on one request; the official maximum is unknown (its UI creates one or two).
const MAX_PER_REQUEST: usize = 4;
const MAX_VIRTUAL: usize = 16;

fn hyprctl(args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("hyprctl").args(args).output().map_err(|e| format!("hyprctl unavailable: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("hyprctl {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Headless-output support needs the Wayland engine plus the Hyprland command (niri has no
/// dynamic output creation). Probed once per process.
pub fn supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        cfg!(all(target_os = "linux", feature = "wayland"))
            && !crate::desktop_backend::headless()
            && crate::desktop_backend::detect().ok() == Some(crate::desktop_backend::DesktopBackend::Wayland)
            && std::env::var_os("NIRI_SOCKET").is_none()
            && std::process::Command::new("hyprctl").arg("version")
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
                .status().map_or(false, |s| s.success())
    })
}

/// Same sanity envelope as the monitor inventory parse (wayland::parse_outputs).
fn clamp_mode(w: u32, h: u32) -> (u32, u32) {
    let clamp = |v: u32| v.clamp(2, 16384);
    let (w, h) = (clamp(w), clamp(h));
    if u64::from(w) * u64::from(h) * 4 > 256 * 1024 * 1024 {(1920, 1080)} else {(w, h)}
}

/// Lowest unused UUVIRT name.
fn free_name() -> String {
    let used: std::collections::HashSet<String> =
        crate::wayland::virtuals().into_iter().map(|v| v.output.name).collect();
    (0..).map(|n| format!("{}{n}", crate::wayland::VIRTUAL_PREFIX)).find(|name| !used.contains(name)).expect("bounded by MAX_VIRTUAL")
}

/// Headless outputs present in `hyprctl monitors -j` (enabled, our prefix). Transformed
/// outputs (flip_screen) are kept upright: dims swap for odd transforms, flipped variants
/// 4-7 are unusable, same policy as wayland::parse_outputs.
fn live_outputs() -> Vec<crate::wayland::Output> {
    let prefix = crate::wayland::VIRTUAL_PREFIX;
    let mut live = Vec::new();
    let Ok(raw) = hyprctl(&["monitors", "-j"]) else { return live };
    let Ok(monitors) = serde_json::from_slice::<Vec<serde_json::Value>>(raw.as_bytes()) else { return live };
    for m in monitors {
        let output = (|| {
            if m["disabled"].as_bool() == Some(true) { return None; }
            let transform = m["transform"].as_u64().unwrap_or(0);
            if transform > 3 { return None; }
            let name = m["name"].as_str()?;
            if !name.starts_with(prefix) || name.len() > 127 { return None; }
            let (mut width, mut height) = (u32::try_from(m["width"].as_u64()?).ok()?, u32::try_from(m["height"].as_u64()?).ok()?);
            if transform & 1 == 1 { std::mem::swap(&mut width, &mut height); }
            // Headless outputs report no refresh rate; advertise a desktop default.
            let fps = m["refreshRate"].as_f64().map_or(60, |v| v.round().clamp(1.0, 240.0) as i32);
            if width < 2 || height < 2 || width > 16384 || height > 16384 { return None; }
            Some(crate::wayland::Output { name: name.into(), width, height, fps, x: 0, y: 0 })
        })();
        if let Some(output) = output { live.push(output); }
    }
    live
}

/// `hyprctl output <name> mode` applies asynchronously; wait until the monitor list reports the
/// requested size so the advertised ScreenSources match what capture will actually read.
fn wait_mode(name: &str, width: u32, height: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if live_outputs().iter().any(|o| o.name == name && (o.width, o.height) == (width, height)) { return; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    log::warn!("Virtual display mode not confirmed: {width}x{height}");
}

/// Creates `count` headless outputs sized from the viewer's `local_resolution` list (one entry
/// per requesting screen, in order; extra displays reuse the last entry). Failure removes what
/// it already created, so the advertised list never mentions a half-built set.
pub fn create(local_resolution: &[(u32, u32)], count: i64) -> Result<(), String> {
    if !supported() { return Err("Virtual displays need the Hyprland Wayland backend".into()); }
    let requested = count;
    let count = count.clamp(1, MAX_PER_REQUEST as i64) as usize;
    if local_resolution.len() > MAX_VIRTUAL { return Err("Too many resolutions".into()); }
    if crate::wayland::virtuals().len() >= MAX_VIRTUAL { return Err("Virtual display limit reached".into()); }
    // Physical ids must exist before ours do, even without UU_WAYLAND_MULTISCREEN.
    crate::wayland::force_inventory();
    let mut created: Vec<String> = Vec::new();
    for index in 0..count {
        let (w, h) = local_resolution.get(index).or_else(|| local_resolution.last()).copied().unwrap_or((1920, 1080));
        let (w, h) = clamp_mode(w, h);
        let name = free_name();
        // Hyprland 0.56: `output <name> mode` reports ok but never applies to headless outputs,
        // and the legacy `keyword monitor` IPC is gone ("non-legacy parsers"); the Lua eval is
        // the working path (https://github.com/olafkfreund/nixarchy/issues/1031). Older
        // Hyprland without `eval` falls back to the output command, which worked there.
        let lua = format!("hl.monitor({{ output = \"{name}\", mode = \"{w}x{h}@60\", position = \"auto\", scale = 1 }})");
        let result = hyprctl(&["output", "create", "headless", &name])
            .and_then(|_| hyprctl(&["eval", &lua])
                .or_else(|_| hyprctl(&["output", &name, "mode", &format!("{w}x{h}")])));
        if let Err(error) = result {
            let _ = hyprctl(&["output", "remove", &name]);
            for name in &created { let _ = hyprctl(&["output", "remove", name]); }
            log::warn!("Virtual display create failed at {}/{} (requested {requested}): {error}", index + 1, count);
            refresh();
            return Err(error);
        }
        wait_mode(&name, w, h);
        log::info!("Virtual display created: {}x{} ({}/{})", w, h, index + 1, count);
        created.push(name);
    }
    refresh();
    Ok(())
}

/// Removes one virtual display by advertised screen id. The encoder is moved back to screen 0
/// first when it is reading the output being removed, so the stream survives the removal.
pub fn remove(screen_id: i64) -> Result<(), String> {
    if !supported() { return Err("Virtual displays need the Hyprland Wayland backend".into()); }
    let id = usize::try_from(screen_id).map_err(|_| "Invalid screen id".to_string())?;
    let name = crate::wayland::virtuals().into_iter().find(|v| v.id == id)
        .map(|v| v.output.name).ok_or_else(|| "Not a virtual display".to_string())?;
    if crate::wayland::selected() == id {
        let _ = crate::wayland::select(0);
    }
    hyprctl(&["output", "remove", &name])?;
    refresh();
    log::info!("Virtual display removed: id={}", id);
    Ok(())
}

/// Rebuilds the advertised virtual list from `hyprctl monitors -j`, keeping every live output's
/// id stable (ids of removed displays are only reused after a refresh without them). Also
/// called after a flip_screen rotation so the virtual sizes follow the transform.
pub(crate) fn refresh() {
    let live = live_outputs();
    let previous = crate::wayland::virtuals();
    let mut used: Vec<usize> = previous.iter().map(|v| v.id).collect();
    let mut assigned: Vec<crate::wayland::VirtualOutput> = Vec::new();
    for output in live {
        let id = previous.iter().find(|v| v.output.name == output.name).map(|v| v.id).unwrap_or_else(|| {
            let mut id = crate::wayland::VIRTUAL_BASE;
            while used.contains(&id) { id += 1; }
            used.push(id);
            id
        });
        assigned.push(crate::wayland::VirtualOutput { id, output });
    }
    assigned.sort_by_key(|v| v.id);
    assigned.truncate(MAX_VIRTUAL);
    crate::wayland::set_virtuals(assigned);
}

fn varint(fields: &[WireField], n: u32) -> Option<u64> {
    fields.iter().find_map(|f| match (f.number, &f.value) { (x, WireValue::Varint(v)) if x == n => Some(*v), _ => None })
}
fn message<'a>(fields: &[WireField<'a>], n: u32) -> Option<&'a [u8]> {
    fields.iter().find_map(|f| match (f.number, &f.value) { (x, WireValue::Bytes(b)) if x == n => Some(*b), _ => None })
}

fn response(header: &[u8], case: u64, error_code: u64) -> Vec<u8> {
    let mut body = Vec::new();
    crate::capture_setting::put_varint(1, error_code, &mut body);
    let mut out = Vec::new();
    crate::capture_setting::put_bytes(1, header, &mut out);
    crate::capture_setting::put_bytes(case, &body, &mut out);
    out
}

/// True when one Message 21 payload is a virtual-display request (cases 12/13).
pub fn wants(rpc: &[WireField]) -> bool {
    message(rpc, CREATE_REQUEST).is_some() || message(rpc, REMOVE_REQUEST).is_some()
}

fn decode_create(fields: &[WireField]) -> (Vec<(u32, u32)>, i64) {
    let mut local_resolution = Vec::new();
    for field in fields {
        let WireValue::Bytes(resolution) = &field.value else { continue };
        if field.number != 1 { continue; }
        if let Ok(resolution) = decode_wire(resolution) {
            if let (Some(w), Some(h)) = (varint(&resolution, 1), varint(&resolution, 2)) {
                local_resolution.push((w as u32, h as u32));
            }
        }
    }
    (local_resolution, varint(fields, 2).unwrap_or(1) as i64)
}

/// Handles one Message 21 payload; returns the RpcResponse body (case 8/9 with error_code)
/// when it was a virtual-display request, None otherwise. Blocking (hyprctl).
pub fn handle_rpc(rpc: &[u8]) -> Option<(u64, Vec<u8>)> {
    let fields = decode_wire(rpc).ok()?;
    let header = message(&fields, 1).unwrap_or(&[]).to_vec();
    if let Some(request) = message(&fields, CREATE_REQUEST) {
        let (local_resolution, count) = decode_wire(request).ok().map(|f| decode_create(&f))?;
        let result = create(&local_resolution, count);
        let code = if result.is_ok() { ERROR_OK } else { ERROR_FAILED };
        if let Err(error) = result { log::warn!("Virtual display create rejected: {error}"); }
        return Some((CREATE_RESPONSE, response(&header, CREATE_RESPONSE, code)));
    }
    if let Some(request) = message(&fields, REMOVE_REQUEST) {
        let screen_id = decode_wire(request).ok().and_then(|f| varint(&f, 1)).map(|v| v as i64).unwrap_or(-1);
        let result = if screen_id >= 0 { remove(screen_id) } else { Err("Invalid screen id".into()) };
        let code = if result.is_ok() { ERROR_OK } else { ERROR_FAILED };
        if let Err(error) = result { log::warn!("Virtual display remove rejected: {error}"); }
        return Some((REMOVE_RESPONSE, response(&header, REMOVE_RESPONSE, code)));
    }
    None
}

/// Diagnostic: create one headless output at `resolution`, print the advertised screens, remove
/// it again. Verifies the whole non-RPC path (hyprctl, inventory, ids) on a live Hyprland.
pub fn probe(resolution: (u32, u32)) -> Result<(), String> {
    println!("Virtual display probe: backend supported={}", supported());
    create(&[resolution], 1)?;
    for (id, output) in crate::wayland::screens() {
        println!("screen id={id}: {} {}x{}{}{}", output.name, output.width, output.height,
            if id >= crate::wayland::VIRTUAL_BASE { " (virtual)" } else { "" },
            if id == crate::wayland::selected() { " [current]" } else { "" });
    }
    let last = crate::wayland::virtuals().iter().max_by_key(|v| v.id).map(|v| v.id);
    if let Some(id) = last {
        remove(id as i64)?;
    }
    println!("Virtual display probe: removed; screens now {}", crate::wayland::screens().len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(case: u32, body: &[u8]) -> Vec<u8> {
        let mut header = Vec::new();
        crate::capture_setting::put_varint(1, 55, &mut header);
        let mut out = Vec::new();
        crate::capture_setting::put_bytes(1, &header, &mut out);
        crate::capture_setting::put_bytes(case as u64, body, &mut out);
        out
    }

    #[test]
    fn create_request_decodes_resolutions_and_count() {
        let mut first = Vec::new();
        crate::capture_setting::put_varint(1, 2560, &mut first);
        crate::capture_setting::put_varint(2, 1440, &mut first);
        let mut second = Vec::new();
        crate::capture_setting::put_varint(1, 1920, &mut second);
        crate::capture_setting::put_varint(2, 1080, &mut second);
        let mut body = Vec::new();
        crate::capture_setting::put_bytes(1, &first, &mut body);
        crate::capture_setting::put_bytes(1, &second, &mut body);
        crate::capture_setting::put_varint(2, 2, &mut body);
        let payload = rpc(CREATE_REQUEST, &body);
        let fields = decode_wire(&payload).unwrap();
        assert!(wants(&fields));
        let (resolutions, count) = decode_create(&decode_wire(crate::capture_setting::message(&fields, CREATE_REQUEST).unwrap()).unwrap());
        assert_eq!(resolutions, vec![(2560, 1440), (1920, 1080)]);
        assert_eq!(count, 2);
        // Response body: case 8 {1 error_code 0 = success}.
        let reply = response(&[8, 55], CREATE_RESPONSE, ERROR_OK);
        let reply_fields = decode_wire(&reply).unwrap();
        assert_eq!(crate::capture_setting::message(&reply_fields, 1), Some(&[8, 55][..]));
        let error = decode_wire(crate::capture_setting::message(&reply_fields, 8).unwrap()).unwrap();
        assert_eq!(crate::capture_setting::number(&error, 1).unwrap(), 0);
    }

    #[test]
    fn remove_request_decodes_screen_id() {
        let mut body = Vec::new();
        crate::capture_setting::put_varint(1, 100, &mut body);
        let payload = rpc(REMOVE_REQUEST, &body);
        let fields = decode_wire(&payload).unwrap();
        assert!(wants(&fields));
        let screen_id = decode_wire(crate::capture_setting::message(&fields, REMOVE_REQUEST).unwrap()).unwrap();
        assert_eq!(varint(&screen_id, 1), Some(100));
        let reply = response(&[8, 55], REMOVE_RESPONSE, ERROR_FAILED);
        let reply_fields = decode_wire(&reply).unwrap();
        assert_eq!(crate::capture_setting::message(&reply_fields, 1), Some(&[8, 55][..]));
        let error = decode_wire(crate::capture_setting::message(&reply_fields, 9).unwrap()).unwrap();
        assert_eq!(crate::capture_setting::number(&error, 1).unwrap(), 1);
    }

    #[test]
    fn other_requests_are_ignored() {
        let payload = rpc(2, &[32, 1]); // capture_setting
        assert!(!wants(&decode_wire(&payload).unwrap()));
        assert!(handle_rpc(&payload).is_none());
    }
}
