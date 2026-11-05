# 官方客户端菜单 → 本项目实现路线图

来源：Windows 官方安装包 `uuyc_4.45.1.exe`（NSIS，2026-11-05 解包静态分析）。
一切编号可回溯：功能开关 = 官方云端 `abilitys` 清单（63 项，嵌在 GameViewerServer.exe）；
协议入口 = `gv_pb.*` protobuf 消息（193 个）与 SimpleAction 枚举（30 个 ACTION_TYPE_*，
与 `src/control_protocol.rs::simple_action_name` 的 0–27 完全吻合）；系统级能力 = 安装包
四个内核驱动（IDD 虚拟屏 / ViGEm 手柄 / nevaudio 虚拟声卡 / uuycinput 底层输入）、
GameViewerService.exe（SYSTEM 服务，登录前控制）、GameViewerHealthd.exe（看门狗）。

状态标记：✅ 已实现 · 🆕 本轮新增 · ⬜ 待做 · 🔭 需先实测（连上 Mac viewer 看真实消息再定形状）
· ❌ 不做（明确排除）。

## 声音菜单（音频）

| 菜单项 | 协议入口 | 状态 | 说明 |
|---|---|---|---|
| 听被控端声音（扬声器开关） | SimpleAction 25 speaker_opened / 26 speaker_closed | 🆕 | `audio::set_speaker` 会话中暂停/恢复桌面声编码器，默认开 |
| 静音运行（被控端静音） | 14/18 开、16 关（已实现）+ 虚拟声卡 | ✅ | 不变 |
| 语音（麦克风进被控端） | 23 microphone_opened / 24 microphone_closed；官方 host 侧 `audio_render_start`、`nevaudio_microphone_default/restore`（把控制端语音渲染到被控端扬声器，并/或作为虚拟麦克风供被控端应用使用——游戏语音场景） | 🔭 | 先实测：Mac viewer 开麦后 SDP 音频 m-line 方向、是否来 RTP、是否有重协商。入站 Opus 渲染用 `ffmpeg -f ogg -i pipe: -f pulse`，虚拟麦克风用 module-null-source + loopback |

## 画面菜单（视频）

| 菜单项 | 协议入口 | 状态 | 说明 |
|---|---|---|---|
| 清晰度（标清/高清/超清/原画/自动/自定义码率） | RPC 2 capture_setting（quality 1–6，v4 CUSTOM_BITRATE） | ✅ | |
| 帧率 30/60/90 | capture_setting 字段 1 档位 + 字段 18 fpsCount | ✅ | |
| 帧率 144（fps_144, 2.2.0+） | 同上（fpsCount 自由数值） | ✅ | 选项 4→144 映射与 fpsCount 直通已在（capture_setting.rs），管线无上限；官方门槛只在观看端菜单 |
| 分辨率（跟随本地/远程/档位） | resolution_type 3/2、16/17、5/6 | ✅ | X11 xrandr；Wayland 缺 wlr-output-management 切换（backlog #2） |
| 真彩 4:4:4 | chorma_format=3（v3 CHROMA_SAMPLING） | ✅ | NVENC/x264 4:4:4，VAAPI 回 4:2:0 |
| HDR（screen_hdr, 4.2.2+） | capture_setting v6 HDR 字段 | 🔭 | 需抓包字段形状 + 采集/编码链路评估 |
| 缩放（screen_zoomscale, 4.0.0+） | 未知 | 🔭 | 观看端缩放系数下发？ |
| 屏幕旋转（screen_orientation, 4.16+） | `SendFlipScreenRequest/Response` | ⬜ | |

## 显示菜单（桌面/显示器）

| 菜单项 | 协议入口 | 状态 | 说明 |
|---|---|---|---|
| 显示桌面 | SimpleAction 2 show_desktop（Super+D 注入） | ✅ | |
| 窗口列表（show_windows, 3） | SimpleAction 3 | 🆕 | 与 show_desktop 同一 Super+D 再按一次（Win+D 切换语义） |
| 多显示器 | 现有 screen-switch + UU_X11/WAYLAND_MULTISCREEN | ✅ | |
| 多显示器 | 现有 screen-switch + UU_X11/WAYLAND_MULTISCREEN | ✅ | |
| 虚拟显示器（screen_virtual, 1.2.1+） | RPC 12 `CreateVirtualDisplayRequest{1: rep ScreenResolution, 2: count}` → 响应 8 `{1: error_code}`；13 `RemoveVirtualDisplayRequest{1: screen_id}` → 9（GameViewerServer.exe descriptor 反解） | 🆕 | `src/virtual_display.rs`：Hyprland headless output（`hyprctl output create headless` + 0.56 的 `hyprctl eval hl.monitor` 设分辨率——`output mode`/`keyword` 均无效，见 nixarchy#1031）。虚拟屏 id ≥100 稳定不随物理重建漂移，Screen 消息 `screen_type=1`（ScreenType_Virtual），建/删后重发 Message 7；崩溃遗留的 UUVIRT* 输出重启后自动接管；采集走既有 select/wlr-screencopy 路径（headless 帧已验证，VAAPI 编码 60 帧通过）。`virtual-display-probe [WxH]` 诊断子命令。官方响应 error_code 语义未实测（0=成功按本项目惯例） |
| 超级屏幕（screen_super 4.2.3 / manual+fps 4.27+） | `EnterSuperScreenRequest/Response`、`QuitSuperScreen*`、`EnterSuperScreenFpsNotSupport` | 🔭 | 语义未知（疑似高帧率专属模式），抓包后再定 |
| 副屏串流（second_screen, 4.16+） | 未知 | 🔭 | 与虚拟显示器同根；虚拟显示器已落地，同族的 enter/quit super screen 等抓包后接入 |
| 单窗口串流（windows_display） | `ActiveWindowChange`、`WinRect`、`CaptureConfig`、SimpleAction 5 capture_window | ⬜ | X11 窗口区域采集可行；Wayland 难 |

## 安全菜单

| 菜单项 | 协议入口 | 状态 | 说明 |
|---|---|---|---|
| 远程结束后自动锁屏 | 19 set_auto_lock | ✅ | |
| 自动解锁被控端 | 15 auto_unlock_system（PAM） | ✅ | |
| 锁定被控端 / 重启 / 关机 | 10 / 9 / 12 | ✅ | |
| 隐私屏（screen_privacy 1.2.5+） | RPC 7 + 现有 privacy_screen（含壁纸 4.23+） | ✅ | KDE 增强路径已有 |
| 增强隐私（screen_privacy_enhanced, 4.4.0+） | 官方日志 `auto enhanced privacy failed, fallback to normal privacy` | 🔭 | 语义待抓包 |

## 工具菜单

| 菜单项 | 协议入口 | 状态 | 说明 |
|---|---|---|---|
| 文件传输 | RPC 8（FTP v2） | ✅ | |
| 剪贴板（文本） | RPC 9（ff_clipboard=3） | ✅ | |
| 剪贴板文件 / 拖拽（remote_clipboard_v5/v21） | `ClipboardFileDescriptor*` 全家族、`DragDropOleDrop`、`DragDropAutoSave*` | ⬜ | OLE 协议族大；拖拽落盘可先做（viewer 拖文件→host 保存） |
| 终端 | RPC 28/29 + TERM 帧 | ✅ | |
| 端口映射 | capture_type 9 + `PortMappingFrame` | ✅ | |
| 标注 | RPC 27 draw（v1）；annotation_v2 = 4.40+ | ✅ | v2 差异待抓包 |
| 任务管理器 | SimpleAction 13 show_taskmgr（gnome/plasma/xfce → foot+btop → kitty → xterm+top） | ✅ | Omarchy 机器走 foot+btop |
| 快捷键（key_toggle） | 11 key_toggle | ⬜ | 检查现有 HUD 需求 |

## 设备/连接

| 菜单项 | 协议入口 | 状态 | 说明 |
|---|---|---|---|
| 设备分组 / 备注 | `/api/v1/device/groups/of/my`、`RenameSessionReq/Resp` | ⬜ | 云端 API，host 侧无关 |
| 开机唤醒（WOL） | `/api/v1/device/wake_on_lan/info` + push（UU_WOL_HELPER） | ✅（实验） | |
| 应用列表/启动（remote_app_list 4.18+） | `LaunchApp`、`ShowApp`、`QuickLaunchScan*`、`/api/v1/app/list/of/device` | ⬜ | 扫 .desktop → 云端上报 → 点图标即串流该应用窗口 |

## 架构级（非菜单项，记录差距）

- 登录前/无人值守控制：官方 SYSTEM 服务 + winlogon/SeTcbPrivilege；Linux 无等价路径（KMS greeter 黑屏未解）
- 看门狗：GameViewerHealthd（streamer/main_loop 健康重启）；我们有 systemd Restart=always，缺会话级
- 被控端自升级（controlled_update 2.5+）：官方 `device/update/prepare`
- 卡顿归因（delay_reason 3.1.3+）+ `ReportQosStats`
- 手柄（device_gamepad）：**用户明确排除**
- 网易生态（mumu_control、wow_detection、LaunchCloudPcApp）：不做

## 验证方法（每个 🔭 项）

连上 Mac viewer → 在对应菜单里点开/关闭该功能 → `journalctl --user -u uuremote-native`
看新 action 号 / RPC case / SDP 变化（ground rule 3：只记字段号与形状，不猜）。
未知 SimpleAction 现在就会打出 `SimpleAction N (name) received/not handled: args=…`（形状）。
