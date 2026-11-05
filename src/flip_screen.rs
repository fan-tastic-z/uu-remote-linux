//! 屏幕旋转 / screen rotation (官方 4.16+ screen_orientation 菜单): RpcRequest case 22
//! SendFlipScreenRequest {1: screen_orientation} → RpcResponse case 18 {1 error_code}
//! (GameViewerServer.exe 4.45.1 descriptor). ScreenOrientation: 1 landscape, 2 portrait.
//! The request names no screen: it applies to the output the stream is currently reading
//! (the viewer picks a screen in its display menu, then rotates). Hyprland rotates through the
//! output transform (wlroots enum: 0 normal, 1 = 90° clockwise, 2 180°, 3 270°) — the Lua
//! `hyprctl eval` path, like virtual_display: the `output <name> transform` command reports ok
//! but never applies on 0.56. The engine then reports the upright (swapped) size and screencopy
//! frames arrive upright (verified on Hyprland 0.56.2: SHM copy + VAAPI encode at 1080x1920).
//! The official portrait direction is not observable offline: portrait maps to 90° clockwise;
//! compare against the Mac viewer live and flip the constant if it disagrees.

use crate::protocol::{decode_wire, WireField};

pub const FLIP_REQUEST: u32 = 22;
const FLIP_RESPONSE: u64 = 18;
const ERROR_OK: u64 = 0;
const ERROR_FAILED: u64 = 1;
/// Official ScreenOrientation (gv_pb enum): 1 landscape, 2 portrait; 0 unspecified.
pub const LANDSCAPE: u64 = 1;
pub const PORTRAIT: u64 = 2;

fn hyprctl(args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("hyprctl").args(args).output().map_err(|e| format!("hyprctl unavailable: {e}"))?;
    if !output.status.success() {
        return Err(format!("hyprctl {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn varint(fields: &[WireField], n: u32) -> Option<u64> {
    fields.iter().find_map(|f| match (f.number, &f.value) { (x, crate::protocol::WireValue::Varint(v)) if x == n => Some(*v), _ => None })
}
fn message<'a>(fields: &[WireField<'a>], n: u32) -> Option<&'a [u8]> {
    fields.iter().find_map(|f| match (f.number, &f.value) { (x, crate::protocol::WireValue::Bytes(b)) if x == n => Some(*b), _ => None })
}

/// The transform one output currently reports in `hyprctl monitors -j`, if present.
fn current_transform(name: &str) -> Option<u64> {
    let raw = hyprctl(&["monitors", "-j"]).ok()?;
    let monitors = serde_json::from_slice::<Vec<serde_json::Value>>(raw.as_bytes()).ok()?;
    monitors.into_iter().find(|m| m["name"].as_str() == Some(name))?["transform"].as_u64()
}

/// Rotates one output to `orientation` and rebuilds the inventory so the advertised size
/// follows. Blocking (hyprctl). The capture restart is the caller's generation bump.
pub fn apply_to(name: &str, orientation: u64) -> Result<(), String> {
    if !crate::virtual_display::supported() { return Err("Screen rotation needs the Hyprland Wayland backend".into()); }
    let transform = if orientation == PORTRAIT {1} else {0};
    let lua = format!("hl.monitor({{ output = \"{name}\", transform = {transform} }})");
    hyprctl(&["eval", &lua])?;
    // Applied asynchronously; wait until the monitor list agrees before re-advertising sizes.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if current_transform(name) == Some(transform) { break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if current_transform(name) != Some(transform) { return Err("Rotation not confirmed".into()); }
    // Physical list via force_inventory; the virtual list refreshes from hyprctl too, so a
    // rotated headless display re-advertises its upright size.
    crate::wayland::force_inventory();
    crate::virtual_display::refresh();
    log::info!("Screen rotated: orientation={orientation} transform={transform}");
    Ok(())
}

/// True when one Message 21 payload is a flip-screen request (case 22).
pub fn wants(rpc: &[WireField]) -> bool { message(rpc, FLIP_REQUEST).is_some() }

/// Handles one Message 21 payload; returns (response case 18, body, changed) when it was a
/// flip request. `changed` is true when the transform moved and the caller must restart
/// capture and re-advertise screens. Blocking (hyprctl).
pub fn handle_rpc(rpc: &[u8]) -> Option<(u64, Vec<u8>, bool)> {
    let fields = decode_wire(rpc).ok()?;
    let header = message(&fields, 1).unwrap_or(&[]).to_vec();
    let request = message(&fields, FLIP_REQUEST)?;
    let orientation = decode_wire(request).ok().and_then(|f| varint(&f, 1)).unwrap_or(0);
    let result = match crate::wayland::source(crate::wayland::selected()) {
        Some(output) => apply_to(&output.name, orientation),
        None => Err("No current output".into()),
    };
    if let Err(error) = &result { log::warn!("Screen rotation rejected: {error}"); }
    let mut body = Vec::new();
    crate::capture_setting::put_varint(1, if result.is_ok() { ERROR_OK } else { ERROR_FAILED }, &mut body);
    let mut out = Vec::new();
    crate::capture_setting::put_bytes(1, &header, &mut out);
    crate::capture_setting::put_bytes(FLIP_RESPONSE, &body, &mut out);
    Some((FLIP_RESPONSE, out, result.is_ok()))
}

/// Diagnostic on a throwaway headless output: create, rotate portrait (expect the swapped
/// size in the advertised screens), rotate back, remove. Never touches a physical monitor.
pub fn probe() -> Result<(), String> {
    println!("Screen flip probe: backend supported={}", crate::virtual_display::supported());
    crate::virtual_display::create(&[(1600, 900)], 1)?;
    let name = crate::wayland::virtuals().iter().max_by_key(|v| v.id)
        .map(|v| v.output.name.clone()).ok_or("No virtual display to rotate")?;
    for (label, orientation) in [("portrait", PORTRAIT), ("landscape", LANDSCAPE)] {
        apply_to(&name, orientation)?;
        let size = crate::wayland::screens().into_iter().find(|(_, o)| o.name == name)
            .map(|(_, o)| (o.width, o.height)).ok_or("Rotated output missing from screens")?;
        println!("{label}: advertised {size:?}");
    }
    crate::virtual_display::remove(crate::wayland::virtuals().iter().max_by_key(|v| v.id).map(|v| v.id).unwrap() as i64)?;
    println!("Screen flip probe done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(body: &[u8]) -> Vec<u8> {
        let mut header = Vec::new();
        crate::capture_setting::put_varint(1, 77, &mut header);
        let mut out = Vec::new();
        crate::capture_setting::put_bytes(1, &header, &mut out);
        crate::capture_setting::put_bytes(FLIP_REQUEST as u64, body, &mut out);
        out
    }

    #[test]
    fn flip_request_decodes_orientation() {
        let mut body = Vec::new();
        crate::capture_setting::put_varint(1, PORTRAIT, &mut body);
        let payload = rpc(&body);
        let fields = decode_wire(&payload).unwrap();
        assert!(wants(&fields));
        let request = decode_wire(crate::capture_setting::message(&fields, FLIP_REQUEST).unwrap()).unwrap();
        assert_eq!(varint(&request, 1), Some(PORTRAIT));
    }
}
