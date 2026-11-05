//! The host window on nsh/GTK4 (`nsh-ui` feature): this fork's Omarchy-native replacement
//! for the GTK3 C window (`c_engine/assist_window.c`, not built under this feature — GTK3
//! and GTK4 cannot share one process, their C symbols clash). The window owns its GTK4
//! header bar (Hyprland draws no server-side decorations, which is why the C window showed
//! up frameless with no close button), and its palette is the active Omarchy theme
//! (`ui::theme`). Same threading model as the C window: one dedicated GTK thread; the host
//! marshals state onto it through the glib main context and polls the command queue.
//!
//! `ui::bridge` provides the `uu_*` C-ABI functions src/assist.rs and src/login_ui.rs
//! declare, so no call site changes.

pub mod app;
mod bridge;
mod state;
pub mod theme;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};

use nsh_platform_linux as platform;
use platform::{AppOptions, AppView};

static STARTED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static CLOSED: AtomicBool = AtomicBool::new(false);
static HIDE_ON_CLOSE: AtomicBool = AtomicBool::new(false);

static INITIAL_SECTIONS: OnceLock<state::Sections> = OnceLock::new();
static PREVIEW: AtomicBool = AtomicBool::new(false);
/// Signalled when the UI thread's main loop ends (the last window closed); `preview` blocks
/// on it — a process exits when `main` returns, which would kill the fresh window.
static UI_EXIT: Mutex<Option<mpsc::Receiver<()>>> = Mutex::new(None);

/// The sections the window opened with (service: every card; `login`: account only).
pub(crate) fn initial_sections() -> state::Sections {
    *INITIAL_SECTIONS.get().unwrap_or(&state::Sections { assist: true, account: true, settings: true })
}

thread_local! {
    static APP: RefCell<Option<platform::gtk4::Application>> = const { RefCell::new(None) };
    /// The GTK window, on the UI thread only.
    static WINDOW: RefCell<Option<platform::gtk4::ApplicationWindow>> = const { RefCell::new(None) };
}

/// Runs `f` on the UI thread's main loop; dropped before the window exists or after it closed.
pub(crate) fn run_on_ui_thread(f: impl FnOnce() + Send + 'static) {
    if READY.load(Ordering::SeqCst) && !CLOSED.load(Ordering::SeqCst) {
        platform::gtk4::glib::MainContext::default().invoke(f);
    }
}

/// `uuremote-server ui-preview`: the real window with mock state, for iterating on the UI
/// without the host service. Blocks until the window closes.
pub fn preview() {
    PREVIEW.store(true, Ordering::SeqCst);
    let _ = start_with(state::Sections { assist: true, account: true, settings: true }, false, false);
    // main must not return while the window is up (that would kill the UI thread).
    let exit = UI_EXIT.lock().ok().and_then(|mut slot| slot.take());
    if let Some(exit) = exit { let _ = exit.recv(); }
}

/// The assist host window (service: every card, `hidden` until `present`).
pub fn start(hidden: bool, hide_on_close: bool) -> bool {
    start_with(state::Sections { assist: true, account: true, settings: true }, hidden, hide_on_close)
}

/// Standalone `login`: the account card only; closing the window stops it.
pub fn start_login() -> bool {
    start_with(state::Sections { assist: false, account: true, settings: false }, false, false)
}

fn start_with(sections: state::Sections, hidden: bool, hide_on_close: bool) -> bool {
    if STARTED.swap(true, Ordering::SeqCst) {
        let patch = if sections.assist { state::Patch::ShowAssist } else { state::Patch::ShowAccount };
        state::apply(patch);
        return READY.load(Ordering::SeqCst);
    }
    HIDE_ON_CLOSE.store(hide_on_close, Ordering::SeqCst);
    INITIAL_SECTIONS.set(sections).ok();
    if PREVIEW.load(Ordering::SeqCst) {
        // Mock state for iterating on the UI; the engine bridge applies it after the
        // first render installs the signal handles. The QR is a deterministic fake
        // pattern so the preview exercises the same decode path without ever holding
        // a real login credential.
        state::apply(state::Patch::Device(state::Device {
            id: "233 814 729".into(),
            code: "836 217".into(),
            code_visible: true,
            allowed: true,
        }));
        state::apply(state::Patch::Account(state::Account {
            user: None,
            status: crate::i18n::s("预览：扫码登录状态文案", "Preview: sign-in status line").to_owned(),
            error: false,
        }));
        state::apply(state::Patch::Qr(bridge::qr_png_uri(preview_qr_pixels(), PREVIEW_QR_EDGE)));
        state::apply(state::Patch::Settings(settings_snapshot()));
    } else {
        state::apply(state::Patch::Settings(settings_snapshot()));
    }
    let (tx, rx) = mpsc::channel::<bool>();
    let (exit_tx, exit_rx) = mpsc::channel::<()>();
    if let Ok(mut slot) = UI_EXIT.lock() { *slot = Some(exit_rx); }
    let spawned = std::thread::Builder::new().name("uu-ui".into()).spawn(move || {
        if !build_window(hidden, tx) { return; }
        // Runs until the last window closes; the handshake below unblocks inside activate.
        use platform::gtk4::prelude::*;
        APP.with(|cell| {
            if let Some(app) = cell.borrow().as_ref() { let _ = app.run_with_args::<&str>(&[]); }
        });
        let _ = exit_tx.send(());
    });
    match spawned {
        Ok(_) => rx.recv().unwrap_or(false),
        Err(error) => {
            log::error!("UI thread spawn failed: {error}");
            STARTED.store(false, Ordering::SeqCst);
            false
        }
    }
}

/// Builds the GTK application on the calling thread; `true` when the window was created.
fn build_window(hidden: bool, ready: mpsc::Sender<bool>) -> bool {
    use nsh_core::element::Root;
    use nsh_engine::App;
    use platform::gtk4 as gtk;
    use gtk::{glib, prelude::*};

    // gtk4-rs 0.11's startup hook asserts C-side initialization; an embedded window
    // initializes GTK itself, and a failure here says why (no usable display).
    if let Err(error) = gtk::init() {
        log::error!("GTK init failed: {error:?}");
        let _ = ready.send(false);
        return false;
    }
    // GApplication ids need a dot; the Wayland app_id Hyprland window rules match is this
    // string. NON_UNIQUE and empty argv mirror nsh's run_app: no bus name ownership, and
    // GTK never sees the host's own arguments.
    glib::set_prgname(Some("org.uuremote.Server"));
    let app = gtk::Application::builder()
        .application_id("org.uuremote.Server")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| {
        let title = crate::i18n::s("UU 远程 Linux 主机（非官方）", "UU Remote Linux host (unofficial)");
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(title)
            .default_width(460)
            .default_height(760)
            .icon_name("uu-remote-linux") // contrib desktop icon, when installed
            .build();
        // Service mode: closing only hides the window (assistance keeps its own switch);
        // otherwise closing stops whatever the window serves (assist/login). The destroy
        // handler below also owns the AppView, like nsh's run_app: dropping it would stop
        // the engine and the surface would stop drawing.
        if HIDE_ON_CLOSE.load(Ordering::SeqCst) {
            window.connect_close_request(|w| {
                w.set_visible(false);
                glib::Propagation::Stop
            });
        }
        let view = Rc::new(AppView::new(AppOptions {
            app_id: "uu-remote-linux".to_owned(),
            title: title.to_owned(),
            size: (460, 760),
            ..Default::default()
        }));
        if let Some(json) = theme::omarchy_overrides() {
            if let Err(error) = view.set_theme_overrides(Some(&json)) {
                log::warn!("Omarchy theme overrides rejected: {error}");
            }
        }
        view.load(App::Rust(Root::of::<app::HostApp>()));
        window.set_child(Some(&view.widget()));
        let keep = RefCell::new(Some(view));
        window.connect_destroy(move |_| {
            if let Some(view) = keep.borrow_mut().take() { view.stop(); }
            CLOSED.store(true, Ordering::SeqCst);
            state::push(state::Command::Stop);
        });
        WINDOW.with(|cell| *cell.borrow_mut() = Some(window.clone()));
        if !hidden { window.present(); }
        READY.store(true, Ordering::SeqCst);
        let _ = ready.send(true);
    });
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    true
}

/// Shows and focuses the window (present maps a hidden window too).
pub fn present() {
    run_on_ui_thread(|| {
        use platform::gtk4::prelude::*;
        WINDOW.with(|cell| {
            if let Some(window) = cell.borrow().as_ref() {
                window.present();
            }
        });
    });
}

/// Destroys the window (the service is shutting down).
pub fn close() {
    run_on_ui_thread(|| {
        use platform::gtk4::prelude::*;
        WINDOW.with(|cell| {
            if let Some(window) = cell.borrow_mut().take() { window.destroy(); }
        });
    });
}

/// Copies `value` into the session clipboard (the window's); UI thread only.
pub(crate) fn copy_to_clipboard(value: &str) {
    use platform::gtk4::prelude::*;
    WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            window.clipboard().set_text(value);
        }
    });
}

/// The stored preferences plus the effective selection, for the 设置 card.
pub(crate) fn settings_snapshot() -> state::Settings {
    use crate::video_select::{self, CodecPref};
    let stored = crate::system_actions::load_settings();
    let selection = video_select::selection();
    let codec = match selection.codec_pref {
        CodecPref::Auto => "auto",
        CodecPref::H264 => "h264",
        CodecPref::H265 => "h265",
    };
    state::Settings {
        encoder_pref: stored.video_encoder.clone(),
        codec_pref: stored.video_codec.clone(),
        encoder_effective: format!(
            "{} · {}",
            format!("{:?}", selection.encoder).to_ascii_lowercase(),
            video_select::encoder_source_label(selection.encoder_source),
        ),
        codec_effective: format!("{} · {}", codec, video_select::source_label(selection.codec_source)),
    }
}

/// Writes one video preference (the same file `set-encoder`/`set-codec` edit), switches
/// live desktop streams the way the SIGUSR2 handler does, and refreshes the card. Runs
/// on the engine thread from a picker tap.
pub(crate) fn store_video_pref(encoder: bool, value: &str) {
    let write = crate::system_actions::merge_save(|settings| {
        if encoder { settings.video_encoder = Some(value.to_owned()); }
        else { settings.video_codec = Some(value.to_owned()); }
    });
    match write {
        Ok(_) => {
            crate::video_select::publish_reload();
            state::apply(state::Patch::Settings(settings_snapshot()));
        }
        Err(error) => log::warn!("storing video preference {value} failed: {error}"),
    }
}

pub fn closed() -> bool {
    CLOSED.load(Ordering::SeqCst)
}

/// Whether the window is live and can be presented.
pub fn ui_ready() -> bool {
    READY.load(Ordering::SeqCst) && !CLOSED.load(Ordering::SeqCst)
}

/// The preview QR's edge in pixels (26 modules at 8 px).
const PREVIEW_QR_EDGE: u32 = 208;

/// A deterministic fake QR pattern — finder squares at three corners, xorshift filler —
/// as RGBA8, the same format `uu_login_ui_set_qr` receives.
fn preview_qr_pixels() -> Vec<u8> {
    let (modules, scale) = (26usize, 8usize);
    let edge = modules * scale;
    let mut rgba = vec![255u8; edge * edge * 4];
    let mut paint = |mx: usize, my: usize| {
        for dy in 0..scale { for dx in 0..scale {
            let offset = ((my * scale + dy) * edge + mx * scale + dx) * 4;
            rgba[offset..offset + 3].fill(0);
        }}
    };
    for i in 0..7 { for j in 0..7 {
        let on = i == 0 || i == 6 || j == 0 || j == 6 || (2..=4).contains(&i) && (2..=4).contains(&j);
        if on {
            paint(j, i); // top left
            paint(modules - 7 + j, i); // top right
            paint(j, modules - 7 + i); // bottom left
        }
    }}
    let mut seed = 0x2026_1105u32;
    for y in 8..modules - 8 { for x in 8..modules - 8 {
        seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
        if seed % 5 < 2 { paint(x, y); }
    }}
    rgba
}
