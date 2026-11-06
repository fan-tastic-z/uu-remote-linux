//! The window's content: one narrow column of cards in the active theme (Omarchy's palette
//! through `ui::theme`). Bilingual strings follow the project's `s(zh, en)` convention.

use nsh_base::{button, switch, theme};
use nsh_core::element::View;
use nsh_core::prelude::*;

use super::state::{self, Command};
use std::sync::atomic::Ordering;

fn s(zh: &'static str, en: &'static str) -> &'static str {
    crate::i18n::s(zh, en)
}

/// The app's root: installs the signal handles, applies the opening sections, draws the
/// whole surface on the theme's background.
#[component]
pub fn HostApp() -> impl IntoElement {
    state::start_bridge(state::Handles {
        device: global::<state::Device>(),
        account: global::<state::Account>(),
        problem: global::<state::Problem>(),
        sections: global::<state::Sections>(),
        qr: global::<state::Qr>(),
        settings: global::<state::Settings>(),
    });
    // The engine defaults every section to hidden; equal writes do not notify.
    state::apply_now(state::Patch::Sections(super::initial_sections()));
    view()
        .flex_1()
        .bg(move || theme().background)
        .child(
            view()
                .flex_1()
                .p_3()
                .gap_3()
                .child(problem_banner())
                .child(when(move || state::Sections::get().assist, device_card))
                .child(when(move || state::Sections::get().account, account_card))
                .child(when(move || state::Sections::get().settings, settings_card))
                .child(footer()),
        )
}

fn card() -> View {
    view()
        .bg(move || theme().card)
        .border_1()
        .border_color(move || theme().border)
        .rounded(px(12.0))
}

fn key(label: &'static str) -> impl IntoElement {
    text(s(label, label)).text_color(move || theme().muted_foreground)
}

fn problem_banner() -> impl IntoElement {
    when(move || state::Problem::get().text.is_some(), || {
        view()
            .p_4()
            .rounded(px(12.0))
            .bg(move || theme().destructive.opacity(0.12))
            .border_1()
            .border_color(move || theme().destructive.opacity(0.4))
            .child(
                text(state::Problem::get().text.unwrap_or_default())
                    .text_color(move || theme().destructive),
            )
    })
}

fn device_card() -> impl IntoElement {
    card()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .p_4()
                .border_b_1()
                .border_b_color(move || theme().border)
                .child(
                    text(s("本设备", "This device"))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(view().flex_1())
                .child(key("允许他人远程协助"))
                .child(
                    switch()
                        .checked(move || state::Device::get().allowed)
                        .accessibility_label("允许他人远程协助")
                        .track_class("w-9 h-5 rounded-full bg-muted checked:bg-primary")
                        .thumb_class("w-4 h-4 rounded-full bg-foreground/60 checked:bg-primary-foreground checked:translate-x-4")
                        .on_change(move |&checked: &bool| {
                            state::Device::set_allowed(checked);
                            state::push(if checked { Command::Allow } else { Command::Deny });
                        }),
                ),
        )
        .child(
            view()
                .p_4()
                .gap_3()
                .child(
                    view().flex_row().items_center().gap_3()
                        .child(key("本设备 ID"))
                        .child(view().flex_1())
                        .child(
                            button()
                                .on_press(move || super::copy_to_clipboard(&device_id_digits(&state::Device::get().id)))
                                .active(|style| style.opacity(0.6))
                                .child(text(s("复制", "Copy")).text_color(move || theme().primary)),
                        ),
                )
                .child(
                    text(move || {
                        let id = state::Device::get().id;
                        if id.is_empty() { s("获取中…", "Loading…").to_owned() } else { id }
                    })
                    .font_weight(FontWeight::BOLD)
                    .text_color(move || theme().foreground),
                )
                .child(
                    view().gap_2().mt_2()
                        .child(key("临时验证码"))
                        .child(
                            view().flex_row().items_center().gap_3()
                                .child(
                                    text(move || {
                                        let device = state::Device::get();
                                        if device.code.is_empty() || !device.code_visible { "--------".to_owned() } else { device.code }
                                    })
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(move || theme().foreground),
                                )
                                .child(
                                    button()
                                        .on_press(state::Device::toggle_code_visible)
                                        .active(|style| style.opacity(0.6))
                                        .child(text(move || {
                                            if state::Device::get().code_visible { s("隐藏", "Hide").to_owned() } else { s("显示", "Show").to_owned() }
                                        })
                                            .text_color(move || theme().primary)),
                                ),
                        ),
                )
                .child(
                    text(s("每次远控结束刷新", "Refreshed after every remote session"))
                        .text_color(move || theme().muted_foreground)
                        .mt_2(),
                ),
        )
}

fn account_card() -> impl IntoElement {
    card()
        .p_4()
        .gap_3()
        .child(
            text(s("账号", "Account"))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        // Signed in: avatar, user name, sign out. Both branches read the Account signal
        // through `when`, so sign-in and sign-out swap them reactively.
        .child(when(move || state::Account::get().user.is_some(), signed_in_row))
        .child(when(move || state::Account::get().user.is_none(), signed_out_flow))
        .child(when(move || !state::Account::get().status.is_empty(), || {
            let account = state::Account::get();
            text(account.status).text_color(move || {
                if state::Account::get().error { theme().destructive } else { theme().muted_foreground }
            })
        }))
        .child(when(move || state::REFRESH_OFFERED.load(Ordering::SeqCst), || {
            button()
                .on_press(|| {
                    state::REFRESH_OFFERED.store(false, Ordering::SeqCst);
                    state::REFRESH_REQUESTED.store(true, Ordering::SeqCst);
                    state::Account::touch(); // re-render hides the button
                })
                .active(|style| style.opacity(0.6))
                .child(text(s("刷新二维码", "Refresh QR code")).text_color(move || theme().primary))
        }))
}

fn signed_in_row() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            view()
                .w(px(40.0))
                .h(px(40.0))
                .rounded(px(20.0))
                .bg(move || theme().primary)
                .items_center()
                .justify_center()
                .child(text(move || {
                    state::Account::get().user.unwrap_or_default().chars().next().map(String::from).unwrap_or_default()
                })
                    .text_color(move || theme().primary_foreground)
                    .font_weight(FontWeight::SEMIBOLD)),
        )
        .child(
            view().flex_1().gap_1()
                .child(text(move || state::Account::get().user.unwrap_or_default())
                    .font_weight(FontWeight::SEMIBOLD).text_color(move || theme().foreground))
                .child(text(s("已登录 · 本机已出现在你的设备列表中", "Signed in · this computer is in your device list"))
                    .text_color(move || theme().muted_foreground)),
        )
        .child(
            button()
                .on_press(move || state::push(Command::Logout))
                .active(|style| style.opacity(0.6))
                .child(text(s("退出登录", "Sign out")).text_color(move || theme().destructive)),
        )
}

fn signed_out_flow() -> impl IntoElement {
    view()
        .gap_3()
        .child(
            view().flex_row().gap_4().items_center()
                .child(
                    // The QR (a credential) renders from its data URI; until the login
                    // flow sends one, the pad explains itself.
                    view()
                        .w(px(160.0))
                        .h(px(160.0))
                        .rounded(px(12.0))
                        .bg(rgb(0xffffff))
                        .items_center()
                        .justify_center()
                        .child(when(move || state::Qr::get().uri.is_some(), || {
                            image(move || state::Qr::get().uri.unwrap_or_default())
                                .w(px(160.0))
                                .h(px(160.0))
                        }))
                        .child(when(move || state::Qr::get().uri.is_none(), || {
                            text(s("二维码加载后显示于此", "The QR code shows here")).text_color(rgb(0x9893a5))
                        })),
                )
                .child(
                    view().gap_2()
                        .child(text(s("扫码登录", "Scan to sign in")).font_weight(FontWeight::SEMIBOLD)
                            .text_color(move || theme().foreground))
                        .child(text(s("请使用网易 UU 远程 App 扫一扫登录；远程协助无需登录也可使用。",
                            "Scan with the NetEase UU Remote app; remote assistance also works signed out."))
                            .text_color(move || theme().muted_foreground)),
                ),
        )
}

fn footer() -> impl IntoElement {
    text(s("开源 Linux 被控端 · 非网易官方客户端", "Open-source Linux host · not an official NetEase client"))
        .text_color(move || theme().muted_foreground)
}

/// One picker choice: primary when it is the stored preference.
fn pref_choice(zh: &'static str, en: &'static str, value: &'static str, encoder: bool) -> impl IntoElement {
    button()
        .on_press(move || super::store_video_pref(encoder, value))
        .active(|style| style.opacity(0.6))
        .child(text(s(zh, en)).text_color(move || {
            let settings = state::Settings::get();
            let current = if encoder { settings.encoder_pref } else { settings.codec_pref };
            if current.as_deref().unwrap_or("auto") == value { theme().primary } else { theme().muted_foreground }
        }))
}

fn settings_card() -> impl IntoElement {
    card()
        .p_4()
        .gap_3()
        .child(
            text(s("设置", "Settings"))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            view().flex_row().items_center().gap_2()
                .child(key("视频编码器"))
                .child(view().flex_1())
                .child(pref_choice("自动", "Auto", "auto", true))
                .child(pref_choice("CPU", "CPU", "cpu", true))
                .child(pref_choice("VAAPI", "VAAPI", "vaapi", true))
                .child(pref_choice("NVENC", "NVENC", "nvenc", true)),
        )
        .child(
            text(move || format!("{}: {}", s("当前生效", "Effective"), state::Settings::get().encoder_effective))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view().flex_row().items_center().gap_2()
                .child(key("视频编码"))
                .child(view().flex_1())
                .child(pref_choice("自动", "Auto", "auto", false))
                .child(pref_choice("H.264", "H.264", "h264", false))
                .child(pref_choice("H.265", "H.265", "h265", false)),
        )
        .child(
            text(move || format!("{}: {}", s("当前生效", "Effective"), state::Settings::get().codec_effective))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view().flex_row().items_center().gap_3().mt_2()
                .child(key("诊断"))
                .child(view().flex_1())
                .child(
                    // The subcommand's report is paste-safe by design (booleans, names,
                    // versions); a child process keeps the engine thread free.
                    button()
                        .on_press(|| {
                            std::thread::spawn(|| {
                                let report = std::process::Command::new("/proc/self/exe").arg("doctor").output()
                                    .ok()
                                    .filter(|output| output.status.success())
                                    .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
                                if let Some(report) = report {
                                    super::run_on_ui_thread(move || super::copy_to_clipboard(&report));
                                }
                            });
                        })
                        .active(|style| style.opacity(0.6))
                        .child(text(s("复制报告", "Copy report")).text_color(move || theme().primary)),
                ),
        )
}

/// The ID is copied without the display spaces, so it pastes into the viewer's ID field.
fn device_id_digits(id: &str) -> String {
    id.chars().filter(|c| !c.is_whitespace()).collect()
}
