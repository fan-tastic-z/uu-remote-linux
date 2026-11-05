//! The window's state as nsh signals, and the command queue the host polls — the same shape
//! as the C window's `uu_assist_ui_take_command` FIFO, so src/assist.rs's handling carries
//! over. Signal handles are `Copy`; the root component installs them on the UI thread
//! ([`install`]), and the host-side bridge marshals updates onto that thread
//! ([`super::run_on_ui_thread`]) before writing.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use nsh_core::prelude::*;

/// A user action, mirroring the C window's CMD_* codes; the bridge reports
/// [`Command::code`] and [`Command::arg`] exactly as src/assist.rs expects.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Allow,
    Deny,
    Reset,
    Stop,
    Verify(u8),
    Refresh(u8),
    Custom(String),
    Logout,
    Quit,
}

impl Command {
    pub fn code(&self) -> libc::c_int {
        match self {
            Self::Allow => 1,
            Self::Deny => 2,
            Self::Reset => 3,
            Self::Stop => 4,
            Self::Verify(_) => 5,
            Self::Refresh(_) => 6,
            Self::Custom(_) => 7,
            Self::Logout => 8,
            Self::Quit => 9,
        }
    }
    pub fn arg(&self) -> String {
        match self {
            Self::Verify(v) | Self::Refresh(v) => v.to_string(),
            Self::Custom(code) => code.clone(),
            _ => String::new(),
        }
    }
}

static QUEUE: Mutex<VecDeque<Command>> = Mutex::new(VecDeque::new());

pub fn push(command: Command) {
    if let Ok(mut queue) = QUEUE.lock() {
        if queue.len() < 16 { queue.push_back(command); }
    }
}

/// Pops the next command, if any.
pub fn take_command() -> Option<Command> {
    QUEUE.lock().ok().and_then(|mut queue| queue.pop_front())
}

/// Which cards the window shows: the host service shows all; `login` starts with the account.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Sections {
    pub assist: bool,
    pub account: bool,
    pub settings: bool,
}

/// The 设置 card: the stored encoder/codec preference and the effective selection a
/// desktop stream would use now (e.g. "vaapi · setting").
#[derive(Clone, Default, PartialEq)]
pub struct Settings {
    /// The stored preference; `None` is 自动 (falls through to environment/auto).
    pub encoder_pref: Option<String>,
    pub codec_pref: Option<String>,
    pub encoder_effective: String,
    pub codec_effective: String,
}

/// The 本设备 card.
#[derive(Clone, Default, PartialEq)]
pub struct Device {
    pub id: String,
    pub code: String,
    pub code_visible: bool,
    pub allowed: bool,
}

/// The 账号 card: signed-in user, or the QR sign-in flow's status line.
#[derive(Clone, Default, PartialEq)]
pub struct Account {
    pub user: Option<String>,
    pub status: String,
    pub error: bool,
}

/// A problem only the person at the desktop can fix (`None`: none).
#[derive(Clone, Default, PartialEq)]
pub struct Problem {
    pub text: Option<String>,
}

/// The login QR as a `data:image/png;base64` URI — a credential until it expires: it goes
/// to the window, never into a log. `None` is the placeholder pad.
#[derive(Clone, Default, PartialEq)]
pub struct Qr {
    pub uri: Option<String>,
}

/// A state update from the host, applied on the UI thread.
pub enum Patch {
    /// The whole 本设备 card (the visible-code flag is kept from the current value).
    Device(Device),
    Allowed(bool),
    Account(Account),
    Problem(Option<String>),
    Sections(Sections),
    /// The login QR's data URI (`None`: back to the placeholder pad).
    Qr(Option<String>),
    /// The whole 设置 card (stored prefs + effective selection).
    Settings(Settings),
    /// Reveals the 本设备 card (a second `start`).
    ShowAssist,
    /// Reveals the 账号 card (login attached to the running window).
    ShowAccount,
}

#[derive(Clone, Copy)]
pub(crate) struct Handles {
    pub device: Signal<Device>,
    pub account: Signal<Account>,
    pub problem: Signal<Problem>,
    pub sections: Signal<Sections>,
    pub qr: Signal<Qr>,
    pub settings: Signal<Settings>,
}

thread_local! {
    /// The root component's signal handles, on the engine thread: signal writes go
    /// through a thread-local graph pointer there (`Signal::set`), nowhere else.
    static HANDLES: RefCell<Option<Handles>> = const { RefCell::new(None) };
}

/// Patches from the host threads, waiting for the engine bridge to apply them.
static PENDING: Mutex<VecDeque<Patch>> = Mutex::new(VecDeque::new());
static BRIDGE: AtomicBool = AtomicBool::new(false);

/// Queues `patch` from any thread; the bridge task applies it on the engine thread
/// within ~50 ms.
pub fn apply(patch: Patch) {
    if let Ok(mut pending) = PENDING.lock() { pending.push_back(patch); }
}

/// Applies `patch` now — on the engine thread (inside a render, a callback or the
/// bridge task); before the first render installed the handles it does nothing.
pub(crate) fn apply_now(patch: Patch) {
    HANDLES.with(|cell| {
        let Some(h) = *cell.borrow() else { return };
        match patch {
            Patch::Device(device) => h.device.update(|current| {
                let code_visible = current.code_visible;
                *current = device;
                current.code_visible = code_visible;
            }),
            Patch::Allowed(allowed) => h.device.update(|d| d.allowed = allowed),
            Patch::Account(account) => h.account.set(account),
            Patch::Problem(text) => h.problem.set(Problem { text }),
            Patch::Qr(uri) => h.qr.set(Qr { uri }),
            Patch::Settings(settings) => h.settings.set(settings),
            Patch::Sections(sections) => h.sections.set(sections),
            Patch::ShowAssist => h.sections.update(|s| s.assist = true),
            Patch::ShowAccount => h.sections.update(|s| s.account = true),
        }
    });
}

/// Starts the host→engine bridge from the root component's first run: keeps the signal
/// handles on the engine thread and drains [`PENDING`] there for as long as the
/// surface lives (signal writes from other threads have no graph to write into).
pub(crate) fn start_bridge(handles: Handles) {
    HANDLES.with(|cell| *cell.borrow_mut() = Some(handles));
    if BRIDGE.swap(true, Ordering::SeqCst) { return; }
    spawn(async {
        loop {
            drain();
            timer(std::time::Duration::from_millis(50)).await;
        }
    });
}

/// Applies everything queued, on the engine thread.
fn drain() {
    let mut patches = PENDING.lock().map(|mut pending| std::mem::take(&mut *pending)).unwrap_or_default();
    for patch in patches.drain(..) { apply_now(patch); }
}

impl Sections {
    pub fn get() -> Self {
        global::<Self>().with(|s| *s)
    }
}

impl Device {
    pub fn get() -> Self {
        global::<Self>().with(Clone::clone)
    }
    pub fn set_allowed(allowed: bool) {
        apply_now(Patch::Allowed(allowed));
    }
    pub fn toggle_code_visible() {
        HANDLES.with(|cell| {
            if let Some(h) = *cell.borrow() { h.device.update(|d| d.code_visible = !d.code_visible); }
        });
    }
}

impl Account {
    pub fn get() -> Self {
        global::<Self>().with(Clone::clone)
    }
    /// Notifies readers without changing anything (atomics are not tracked).
    pub fn touch() {
        HANDLES.with(|cell| {
            if let Some(h) = *cell.borrow() { h.account.update(|_| {}); }
        });
    }
}

impl Problem {
    pub fn get() -> Self {
        global::<Self>().with(Clone::clone)
    }
}

impl Qr {
    pub fn get() -> Self {
        global::<Self>().with(Clone::clone)
    }
}

impl Settings {
    pub fn get() -> Self {
        global::<Self>().with(Clone::clone)
    }
}

/// The login QR refresh button: offered by the host, requested by the user.
pub static REFRESH_OFFERED: AtomicBool = AtomicBool::new(false);
pub static REFRESH_REQUESTED: AtomicBool = AtomicBool::new(false);
