//! 应用列表/快速启动 (官方 4.18+ remote_app_list): the viewer's app tab lists the host's
//! applications, shows their icons, and launches or focuses them remotely.
//! Wire shapes from the GameViewerServer.exe 4.45.1 descriptor (gv_pb):
//! - RpcRequest 23 QuickLaunchScanRequest {} → RpcResponse 19 QuickLaunchScanResponse
//!   {1 error_code, 2 error_message, 3 repeated QuickLaunchAppItem
//!    {1 app_key, 2 name, 3 icon_url, 4 target_path, 5 app_id}}.
//! - RpcRequest 24 QuickLaunchAppIconRequest {1 app_key} → RpcResponse 20
//!   {1 error_code, 2 error_message, 3 app_key, 4 bytes icon}.
//! - Message 4 LaunchApp {1 dapp_id, 2 name, 3 target_path, 4 args, 5 app_key} and
//!   Message 5 ShowApp {1 dapp_id, 2 name, 3 target_path, 4 args}: fire-and-forget like
//!   SimpleAction (same outer envelope), no reply.
//! The scan needs no cloud round-trip: the list travels in the RPC response itself. Icons are
//! served per-app from the second RPC (icon_url stays empty; we never upload anything).

use crate::protocol::{decode_wire, WireField, WireValue};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const SCAN_REQUEST: u32 = 23;
pub const ICON_REQUEST: u32 = 24;
const SCAN_RESPONSE: u64 = 19;
const ICON_RESPONSE: u64 = 20;
const ERROR_OK: u64 = 0;
const ERROR_FAILED: u64 = 1;
/// Like the official host's sane bounds; keeps one response comfortably under SCTP limits.
const MAX_APPS: usize = 512;

pub struct AppEntry {
    pub app_key: String,
    pub name: String,
    /// Exec line with desktop field codes (%f %u …) removed; the viewer echoes it back.
    pub exec: String,
    pub wm_class: Option<String>,
    pub icon: Option<String>,
}

fn varint(fields: &[WireField], n: u32) -> Option<u64> {
    fields.iter().find_map(|f| match (f.number, &f.value) { (x, WireValue::Varint(v)) if x == n => Some(*v), _ => None })
}
fn message<'a>(fields: &[WireField<'a>], n: u32) -> Option<&'a [u8]> {
    fields.iter().find_map(|f| match (f.number, &f.value) { (x, WireValue::Bytes(b)) if x == n => Some(*b), _ => None })
}
fn string<'a>(fields: &[WireField<'a>], n: u32) -> Option<&'a str> {
    message(fields, n).and_then(|b| std::str::from_utf8(b).ok())
}

/// Directories scanned in priority order; an earlier desktop id wins over a later duplicate.
fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut push = |p: PathBuf| { if p.is_dir() && !dirs.contains(&p) { dirs.push(p); } };
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        push(home.join(".local/share/applications"));
        push(home.join(".local/share/flatpak/exports/share/applications"));
    }
    push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    push(PathBuf::from("/usr/local/share/applications"));
    push(PathBuf::from("/usr/share/applications"));
    push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs
}

/// Minimal INI: the first [Desktop Entry] group as key → value. Groups after it are skipped.
fn parse_entry(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if in_entry { break; }              // first group past Desktop Entry ends it
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry { continue; }
        if let Some((key, value)) = line.split_once('=') {
            if !key.contains('/') && !key.trim().is_empty() { map.insert(key.trim().to_owned(), value.trim().to_owned()); }
        }
    }
    map
}

/// Name[<lang>] > Name[<lang_short>] > Name, matching the host desktop's locale.
fn localized(map: &HashMap<String, String>, base: &str) -> Option<String> {
    if let Some(lang) = std::env::var("LANG").ok().and_then(|l| l.split('.').next().map(|l| l.to_owned())) {
        if lang.contains('_') {
            if let Some(v) = map.get(&format!("{base}[{lang}]")) { return Some(v.clone()); }
        }
        if let Some(short) = lang.split('_').next() {
            if let Some(v) = map.get(&format!("{base}[{short}]")) { return Some(v.clone()); }
        }
    }
    map.get(base).cloned()
}

/// POSIX-shell word splitting (quotes and backslash), for Exec lines and echoed args.
fn split_exec(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => { has = true; for d in chars.by_ref() { if d == '\'' { break; } cur.push(d); } },
            '"' => {
                has = true;
                while let Some(d) = chars.next() {
                    if d == '"' { break; }
                    if d == '\\' {
                        if let Some(e) = chars.next() {
                            if matches!(e, '"' | '\\' | '$' | '`') { cur.push(e); continue; }
                            cur.push(d); cur.push(e); continue;
                        }
                    }
                    cur.push(d);
                }
            },
            '\\' => { has = true; if let Some(e) = chars.next() { cur.push(e); } },
            c if c.is_whitespace() => { if has { out.push(std::mem::take(&mut cur)); has = false; } },
            c => { has = true; cur.push(c); },
        }
    }
    if has { out.push(cur); }
    out
}

/// Removes desktop entry field codes (%f %F %u %U %d %D %n %N %i %c %k %%).
fn strip_field_codes(mut argv: Vec<String>) -> Vec<String> {
    const CODES: [&str; 12] = ["%f", "%F", "%u", "%U", "%d", "%D", "%n", "%N", "%i", "%c", "%k", "%%"];
    for arg in argv.iter_mut() {
        if CODES.contains(&arg.as_str()) { arg.clear(); }
    }
    argv.retain(|a| !a.is_empty());
    argv
}

/// Scans the standard application directories. Blocking (file I/O).
pub fn scan() -> Vec<AppEntry> {
    let mut by_key: HashMap<String, AppEntry> = HashMap::new();
    for dir in application_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") { continue; }
            let Some(app_key) = path.file_name().and_then(|n| n.to_str()).map(|s| s.to_owned()) else { continue };
            if by_key.contains_key(&app_key) { continue; } // earlier dir wins
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let map = parse_entry(&text);
            if map.get("Type").map(String::as_str) != Some("Application") { continue; }
            if map.get("NoDisplay").map(String::as_str) == Some("true") || map.get("Hidden").map(String::as_str) == Some("true") { continue; }
            let Some(exec_line) = map.get("Exec").cloned() else { continue };
            let argv = strip_field_codes(split_exec(&exec_line));
            if argv.is_empty() { continue; }
            let name = localized(&map, "Name").unwrap_or_else(|| app_key.trim_end_matches(".desktop").to_owned());
            by_key.insert(app_key, AppEntry {
                app_key: path.file_name().unwrap().to_string_lossy().into_owned(),
                name,
                exec: argv.join(" "),
                wm_class: map.get("StartupWMClass").cloned(),
                icon: map.get("Icon").cloned(),
            });
        }
    }
    let mut apps: Vec<AppEntry> = by_key.into_values().collect();
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.app_key.cmp(&b.app_key)));
    apps.truncate(MAX_APPS);
    apps
}

/// Resolves an Icon= value to a PNG (preferred) or SVG, through the freedesktop layout.
pub fn icon_path(icon: &str) -> Option<PathBuf> {
    let value = Path::new(icon);
    if value.is_absolute() { return value.is_file().then(|| value.to_path_buf()); }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".icons"));
        dirs.push(home.join(".local/share/icons"));
    }
    dirs.push(PathBuf::from("/usr/share/icons"));
    let mut try_theme = |theme: &str| -> Option<PathBuf> {
        for dir in &dirs {
            for size in ["128x128", "96x96", "64x64", "48x48", "512x512", "256x256"] {
                let base = dir.join(theme).join(size).join("apps").join(icon);
                let name = base.file_name()?.to_string_lossy().into_owned();
                for ext in ["png", "svg", "xpm"] {
                    let candidate = if name.ends_with(&format!(".{ext}")) { base.clone() }
                        else { base.with_file_name(format!("{name}.{ext}")) };
                    if candidate.is_file() { return Some(candidate); }
                }
            }
            // Scalable SVGs accept bare names and .svg.
            for suffix in [format!("{icon}.svg"), icon.to_owned()] {
                let candidate = dir.join(theme).join("scalable").join("apps").join(suffix);
                if candidate.is_file() { return Some(candidate); }
            }
        }
        None
    };
    try_theme("hicolor").or_else(|| try_theme("Adwaita")).or_else(|| {
        for ext in ["png", "svg", "xpm"] {
            let candidate = PathBuf::from("/usr/share/pixmaps").join(icon).with_extension(ext);
            if candidate.is_file() { return Some(candidate); }
        }
        None
    })
}

/// Message 4 LaunchApp / Message 5 ShowApp share these fields; target_path is our full exec
/// line, args is extra text echoed by the viewer.
fn split_launch(target_path: &str, args: &str) -> Vec<String> {
    let mut argv = strip_field_codes(split_exec(target_path));
    if !args.trim().is_empty() { argv.extend(strip_field_codes(split_exec(args))); }
    argv
}

fn resolve_exe(exe: &str) -> Option<PathBuf> {
    if exe.contains('/') { let p = PathBuf::from(exe); return p.is_file().then_some(p); }
    std::env::var("PATH").ok()?.split(':').map(|d| PathBuf::from(d).join(exe)).find(|p| p.is_file())
}

/// Launches one app in its own user scope so it survives a host restart. The viewer is an
/// authenticated controller of this desktop, so executing what it picks is the official
/// behavior; the executable itself must resolve, args pass through.
pub fn launch(target_path: &str, args: &str) -> Result<(), String> {
    let argv = split_launch(target_path, args);
    let Some(exe) = argv.first().and_then(|e| resolve_exe(e)) else { return Err("Executable not found".into()); };
    let rest = &argv[1..];
    // Own scope (survives host restarts); falls back to a plain child when unavailable.
    let mut command = std::process::Command::new("systemd-run");
    command.args(["--user", "--scope", "--collect", "--quiet", "--"]);
    command.arg(&exe).args(rest);
    match command.spawn() {
        Ok(_) => { log::info!("App launched: exe={}", exe.display()); return Ok(()); }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Launch failed: {error}")),
    }
    let mut direct = std::process::Command::new(&exe);
    direct.args(rest);
    direct.spawn().map_err(|e| format!("Launch failed: {e}"))?;
    log::info!("App launched (direct): exe={}", exe.display());
    Ok(())
}

/// Message 5 ShowApp: focuses the app's window via StartupWMClass on Hyprland. Best effort.
pub fn show(target_path: &str) -> Result<(), String> {
    let wm_class = scan().into_iter()
        .find(|app| app.exec == target_path || app.app_key == target_path)
        .and_then(|app| app.wm_class)
        .ok_or("No StartupWMClass for this app")?;
    let output = std::process::Command::new("hyprctl").args(["dispatch", "focuswindow", &format!("class:^{wm_class}$")]).output()
        .map_err(|e| format!("hyprctl unavailable: {e}"))?;
    if !output.status.success() {
        return Err(format!("focuswindow failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    log::info!("App focused: class={}", wm_class);
    Ok(())
}

fn encode_scan_response(header: &[u8], apps: &[AppEntry]) -> Vec<u8> {
    let mut body = Vec::new();
    crate::capture_setting::put_varint(1, ERROR_OK, &mut body);
    for app in apps {
        let mut item = Vec::new();
        crate::capture_setting::put_bytes(1, app.app_key.as_bytes(), &mut item);
        crate::capture_setting::put_bytes(2, app.name.as_bytes(), &mut item);
        // icon_url stays empty: icons come from the per-app RPC, nothing is uploaded.
        crate::capture_setting::put_bytes(4, app.exec.as_bytes(), &mut item);
        // QuickLaunchAppItem is QuickLaunchScanResponse.apps = field 3 (repeated).
        crate::capture_setting::put_bytes(3, &item, &mut body);
    }
    let mut out = Vec::new();
    crate::capture_setting::put_bytes(1, header, &mut out);
    crate::capture_setting::put_bytes(SCAN_RESPONSE, &body, &mut out);
    out
}

fn encode_icon_response(header: &[u8], app_key: &str, icon: Option<Vec<u8>>) -> Vec<u8> {
    let mut body = Vec::new();
    let code = if icon.is_some() { ERROR_OK } else { ERROR_FAILED };
    crate::capture_setting::put_varint(1, code, &mut body);
    if icon.is_none() {
        let text = "Icon not found";
        crate::capture_setting::put_bytes(2, text.as_bytes(), &mut body);
    }
    crate::capture_setting::put_bytes(3, app_key.as_bytes(), &mut body);
    if let Some(icon) = icon { crate::capture_setting::put_bytes(4, &icon, &mut body); }
    let mut out = Vec::new();
    crate::capture_setting::put_bytes(1, header, &mut out);
    crate::capture_setting::put_bytes(ICON_RESPONSE, &body, &mut out);
    out
}

/// True when one Message 21 payload is an app-list request (cases 23/24).
pub fn wants(rpc: &[WireField]) -> bool {
    message(rpc, SCAN_REQUEST).is_some() || message(rpc, ICON_REQUEST).is_some()
}

/// Handles one Message 21 payload; returns (response case 19/20, body). Blocking (file I/O).
pub fn handle_rpc(rpc: &[u8]) -> Option<(u64, Vec<u8>)> {
    let fields = decode_wire(rpc).ok()?;
    let header = message(&fields, 1).unwrap_or(&[]).to_vec();
    if message(&fields, SCAN_REQUEST).is_some() {
        let apps = scan();
        log::info!("App list scanned: apps={}", apps.len());
        return Some((SCAN_RESPONSE, encode_scan_response(&header, &apps)));
    }
    if let Some(request) = message(&fields, ICON_REQUEST) {
        let fields = decode_wire(request).ok()?;
        let app_key = string(&fields, 1).unwrap_or("");
        let icon = scan().into_iter().find(|app| app.app_key == app_key)
            .and_then(|app| app.icon)
            .and_then(|icon| icon_path(&icon))
            .and_then(|path| std::fs::read(path).ok());
        log::info!("App icon requested: found={}", icon.is_some());
        return Some((ICON_RESPONSE, encode_icon_response(&header, app_key, icon)));
    }
    None
}

/// Message 4 LaunchApp / Message 5 ShowApp payload handling. Blocking (scan for ShowApp).
pub fn handle_message(case: u32, payload: &[u8]) -> Option<Result<(), String>> {
    let fields = decode_wire(payload).ok()?;
    let target_path = string(&fields, 3)?;
    let args = string(&fields, 4).unwrap_or("");
    Some(match case {
        4 => launch(target_path, args),
        _ => show(target_path),
    })
}

/// Diagnostic entry: launch one scanned app by app_key (`app-list-probe --launch <key>`).
pub fn launch_by_key(app_key: &str) -> Result<(), String> {
    let entry = scan().into_iter().find(|app| app.app_key == app_key)
        .ok_or_else(|| format!("app_key {app_key} not found"))?;
    launch(&entry.exec, "")
}

/// Diagnostic: print the scan result (count, first names) and, for one optional app_key,
/// resolve and report the icon size.
pub fn probe(app_key: Option<&str>) -> Result<(), String> {
    let apps = scan();
    println!("App list: {} apps", apps.len());
    for app in apps.iter().take(8) { println!("  {} ({})", app.name, app.app_key); }
    if apps.len() > 8 { println!("  …"); }
    if let Some(key) = app_key {
        let entry = scan().into_iter().find(|app| app.app_key == key).ok_or("app_key not found")?;
        match entry.icon.as_deref().and_then(icon_path) {
            Some(path) => {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!("Icon for {key}: {} ({size} bytes)", path.display());
            }
            None => println!("Icon for {key}: not found"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_parses_localized_name_and_filters() {
        let text = "[Desktop Entry]\nType=Application\nName=Web Browser\nName[zh_CN]=网络浏览器\nExec=firefox %u --new-window\nIcon=firefox\nStartupWMClass=Navigator\n\n[Other]\nName=x\n";
        let map = parse_entry(text);
        assert_eq!(map.get("Type").map(String::as_str), Some("Application"));
        assert_eq!(map.get("Name[zh_CN]").map(String::as_str), Some("网络浏览器"));
        assert_eq!(map.get("StartupWMClass").map(String::as_str), Some("Navigator"));
        // The group after Desktop Entry is not merged.
        assert_eq!(map.get("Name"), Some(&"Web Browser".to_owned()));
    }

    #[test]
    fn exec_splitting_strips_field_codes_and_quotes() {
        assert_eq!(strip_field_codes(split_exec("firefox %u")), vec!["firefox"]);
        assert_eq!(strip_field_codes(split_exec("/usr/bin/foo \"a b\" '%f' --x")),
            vec!["/usr/bin/foo", "a b", "--x"]);
        assert_eq!(split_exec("env A=1 app"), vec!["env", "A=1", "app"]);
    }

    #[test]
    fn scan_response_lists_items_with_the_official_fields() {
        let apps = [AppEntry {
            app_key: "firefox.desktop".into(), name: "Firefox".into(), exec: "/usr/bin/firefox".into(),
            wm_class: Some("Navigator".into()), icon: Some("firefox".into()),
        }];
        let body = encode_scan_response(&[8, 9], &apps);
        let fields = decode_wire(&body).unwrap();
        assert_eq!(crate::capture_setting::message(&fields, 1), Some(&[8, 9][..]));
        let response = decode_wire(crate::capture_setting::message(&fields, SCAN_RESPONSE as u32).unwrap()).unwrap();
        assert_eq!(crate::capture_setting::number(&response, 1), Some(0));
        let item = decode_wire(crate::capture_setting::message(&response, 3).unwrap()).unwrap();
        assert_eq!(crate::capture_setting::message(&item, 1), Some(&b"firefox.desktop"[..]));
        assert_eq!(crate::capture_setting::message(&item, 4), Some(&b"/usr/bin/firefox"[..]));
        // icon_url (3) absent: icons come from RPC 24 only.
        assert!(crate::capture_setting::message(&item, 3).is_none());
    }

    #[test]
    fn scan_request_routes_and_icon_request_decodes() {
        // The routing check is pure: no file I/O in tests (real scan runs hyprctl-free but
        // reads this machine's desktop entries).
        let mut scan_rpc = Vec::new();
        crate::capture_setting::put_bytes(SCAN_REQUEST as u64, &[], &mut scan_rpc);
        assert!(wants(&decode_wire(&scan_rpc).unwrap()));
        let mut icon_request = Vec::new();
        crate::capture_setting::put_bytes(1, b"firefox.desktop", &mut icon_request);
        let mut icon_rpc = Vec::new();
        crate::capture_setting::put_bytes(ICON_REQUEST as u64, &icon_request, &mut icon_rpc);
        assert!(wants(&decode_wire(&icon_rpc).unwrap()));
        let request = decode_wire(crate::capture_setting::message(&decode_wire(&icon_rpc).unwrap(), ICON_REQUEST).unwrap()).unwrap();
        assert_eq!(string(&request, 1), Some("firefox.desktop"));
        let mut other = Vec::new();
        crate::capture_setting::put_bytes(2 as u64, &[32, 1], &mut other);
        assert!(!wants(&decode_wire(&other).unwrap()));
    }

    #[test]
    fn launch_message_decodes_target_and_args() {
        let mut payload = Vec::new();
        crate::capture_setting::put_bytes(2, b"Firefox", &mut payload);
        crate::capture_setting::put_bytes(3, b"firefox --new-window", &mut payload);
        let fields = decode_wire(&payload).unwrap();
        assert_eq!(string(&fields, 3), Some("firefox --new-window"));
    }
}
