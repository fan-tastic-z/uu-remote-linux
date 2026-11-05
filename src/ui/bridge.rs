//! The C-ABI surface src/assist.rs and src/login_ui.rs declare (`extern "C" uu_*`), provided
//! by the nsh window instead of `c_engine/assist_window.c`. Signatures and semantics — the
//! CMD_* codes, the take_command arg buffer, the set_problem return — match the C window so
//! those call sites do not change. Everything marshals onto the UI thread
//! ([`super::run_on_ui_thread`]) before touching the signals, like the C window's
//! `g_idle_add`.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::sync::atomic::Ordering;

use super::state::{self, Account, Device, Patch};
use super::{close, present, start, start_login};

fn text(ptr: *const c_char) -> String {
    if ptr.is_null() { String::new() } else { unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned() }
}

fn copy_arg(arg: *mut c_char, len: usize, value: &str) {
    if arg.is_null() || len == 0 { return; }
    let bytes = value.as_bytes();
    let n = bytes.len().min(len - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), arg as *mut u8, n);
        *arg.add(n) = 0;
    }
}

/// 0 when the window is up (the C window's convention: `== 0` is success at every
/// call site in assist.rs/login_ui.rs), -1 when it failed; a second call reveals the
/// section and reports the live window.
#[no_mangle]
pub extern "C" fn uu_assist_ui_start(hidden: c_int, hide_on_close: c_int) -> c_int {
    if start(hidden != 0, hide_on_close != 0) { 0 } else { -1 }
}

#[no_mangle]
pub extern "C" fn uu_login_ui_start(_name: *const c_char, _os: *const c_char, _cpu: *const c_char, _gpu: *const c_char) -> c_int {
    if start_login() { 0 } else { -1 }
}

#[no_mangle]
pub extern "C" fn uu_assist_ui_present() {
    present();
}

#[no_mangle]
pub extern "C" fn uu_assist_ui_close() {
    close();
}

#[no_mangle]
pub extern "C" fn uu_login_ui_close() {
    close();
}

#[no_mangle]
pub extern "C" fn uu_login_ui_closed() -> c_int {
    c_int::from(super::closed())
}

#[no_mangle]
pub extern "C" fn uu_ui_set_user(name: *const c_char) {
    let name = text(name);
    state::apply(Patch::Account(Account { user: Some(name), ..Account::default() }));
}

#[no_mangle]
pub extern "C" fn uu_assist_ui_set(id: *const c_char, code: *const c_char, allowed: c_int, _verify: c_int, _refresh: c_int, _has_custom: c_int) {
    let device = Device {
        id: text(id),
        code: text(code),
        code_visible: true,
        allowed: allowed != 0,
    };
    state::apply(Patch::Device(device));
}

#[no_mangle]
pub extern "C" fn uu_assist_ui_take_command(arg: *mut c_char, len: usize) -> c_int {
    match state::take_command() {
        Some(command) => {
            copy_arg(arg, len, &command.arg());
            command.code()
        }
        None => {
            copy_arg(arg, len, "");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn uu_login_ui_set_qr(rgba: *const u8, edge: c_int) {
    // The QR image is a login credential (ground rule 2): it goes to the window and the
    // account card, never into a log. Count only on failure.
    if rgba.is_null() || edge <= 0 || edge > 4096 { return; }
    let edge = edge as usize;
    let pixels = unsafe { std::slice::from_raw_parts(rgba, edge * edge * 4) }.to_vec();
    match qr_png_uri(pixels, edge as u32) {
        Some(uri) => state::apply(Patch::Qr(Some(uri))),
        None => log::warn!("QR frame rejected: PNG encode failed, edge={edge}"),
    }
}

/// QR pixels (row-major RGBA8) as a `data:image/png;base64` URI — the form nsh's image
/// loader accepts (`data:` URIs decode without a file or network round trip).
pub(crate) fn qr_png_uri(pixels: Vec<u8>, edge: u32) -> Option<String> {
    use base64::Engine as _;
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, edge, edge);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().ok()?.write_image_data(&pixels).ok()?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
}

#[no_mangle]
pub extern "C" fn uu_login_ui_set_status(status: *const c_char, kind: c_int) {
    let status = text(status);
    let error = kind == 2;
    state::apply(Patch::Account(Account { status, error, ..Account::default() }));
}

#[no_mangle]
pub extern "C" fn uu_login_ui_set_done(name: *const c_char) {
    uu_ui_set_user(name);
}

#[no_mangle]
pub extern "C" fn uu_login_ui_offer_refresh(visible: c_int) {
    state::REFRESH_OFFERED.store(visible != 0, Ordering::SeqCst);
    state::REFRESH_REQUESTED.store(false, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn uu_login_ui_take_refresh() -> c_int {
    c_int::from(state::REFRESH_REQUESTED.swap(false, Ordering::SeqCst))
}

/// Same contract as the C window: 0 notified by a desktop notification, 1 the window was
/// opened instead, -1 neither. `notify` without `text` (clearing the line) is -1.
#[no_mangle]
pub extern "C" fn uu_ui_set_problem(summary: *const c_char, problem: *const c_char, notify: c_int) -> c_int {
    let summary = text(summary);
    let problem = if problem.is_null() { None } else { Some(text(problem)) };
    let line = problem.clone();
    state::apply(Patch::Problem(line));
    if notify == 0 || problem.is_none() { return -1; }
    if notify_desktop(&summary, problem.as_deref().unwrap_or_default()) { return 0; }
    if super::ui_ready() { present(); return 1; }
    -1
}

/// Freedesktop notification over the session bus (critical urgency, like the C window).
fn notify_desktop(summary: &str, body: &str) -> bool {
    use nsh_platform_linux::gtk4::{gio, glib};
    use glib::variant::ToVariant;
    let Ok(connection) = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>) else {
        return false;
    };
    let mut hints = glib::VariantDict::new(None);
    hints.insert_value("urgency", &2u8.to_variant()); // critical: stays until dismissed
    let title = crate::i18n::s("UU 远程 Linux 主机", "UU Remote Linux host");
    let message = glib::Variant::tuple_from_iter([
        title.to_variant(),
        0u32.to_variant(),
        "uu-remote-linux".to_variant(),
        summary.to_variant(),
        body.to_variant(),
        Vec::<String>::new().to_variant(),
        hints.end(),
        (-1i32).to_variant(),
    ]);
    connection
        .call_sync(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            Some(&message),
            None,
            gio::DBusCallFlags::NONE,
            3000,
            None::<&gio::Cancellable>,
        )
        .is_ok()
}

/// The X11 privacy wallpaper is not built under `nsh-ui` (it is a GTK3 unit); the privacy
/// screen falls back to its other covers.
#[no_mangle]
pub extern "C" fn uu_privacy_wallpaper_run(_image: *const c_char, _rects: *const i32, _count: c_int) -> c_int {
    -1
}

/// Whether the window closed on its own (no equivalent C assist call; kept for symmetry).
#[allow(dead_code)]
#[no_mangle]
pub extern "C" fn uu_assist_ui_closed() -> c_int {
    c_int::from(super::closed())
}
