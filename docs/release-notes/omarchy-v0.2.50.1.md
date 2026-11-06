首个 Omarchy 构建：基于上游 [v0.2.50](https://github.com/VallisterDev/uu-remote-linux/releases/tag/v0.2.50)，
加入 nsh 原生主窗口、Hyprland 深度集成与进程内硬件编码，一键脚本安装。/ First Omarchy build: based on upstream
v0.2.50, adding the nsh native window, deep Hyprland integration and in-process hardware encoding, with a
one-line installer.

### 新增 / Added
- **nsh/GTK4 主窗口（`nsh-ui`）**：替换 GTK3 C 窗口，实时跟随 Omarchy 主题换色（含 GTK 窗框），卡片列可滚动，
  quickshell 托盘图标 + 右键菜单（打开主窗口 / 退出），应用启动器入口（`uuremote-server show`）。/ Replaces the
  GTK3 C window, follows the Omarchy theme live (GTK chrome included), scrolls its card column, and adds a
  quickshell tray icon with a right-click menu plus a launcher entry (`uuremote-server show`).
- **Hyprland 集成**：虚拟显示器（headless output）、屏幕旋转（RPC 22）、应用列表（RPC 23/24，systemd-run 启动）、
  144fps 档。/ Virtual display (headless output), screen rotation (RPC 22), app list (RPC 23/24, launched via
  systemd-run), a 144 fps preset.
- **进程内硬件编码（`inproc-av`）**：NVENC / VAAPI 进程内引擎；CPU 编码器只接受 ≤1080p 的 H.265 请求；
  VAAPI 硬件转换显著降低 CPU。/ In-process NVENC / VAAPI engines; CPU encoders only accept H.265 requests up to
  1080p; VAAPI hardware conversion cuts CPU sharply.
- **扫码登录稳定性**：长轮询超时自动重试，二维码不再假性「已失效」。/ The QR-login long poll retries on client
  timeout instead of faking an expired code.

### 下载与安装 / Download
Arch / Omarchy / Hyprland 一键安装 / one-line install:

```sh
curl -fsSL https://raw.githubusercontent.com/fan-tastic-z/uu-remote-linux/omarchy/packaging/omarchy/install.sh | bash
```

或手动下载 `uuremote-omarchy-x86_64.tar.gz`（校验 `.sha256`），解压后运行 `./install.sh`。
预编译二进制在 Arch 滚动版本上构建，仅支持 Arch 系；GNOME/KDE/X11 请用上游版本。/ Or download
`uuremote-omarchy-x86_64.tar.gz` manually (verify with `.sha256`), unpack and run `./install.sh`. The prebuilt
binary targets Arch rolling releases only; for GNOME/KDE/X11 use the upstream release.

### 验证范围 / Validation
单元测试（`cargo test --locked`、`--features wayland,nsh-ui`）与实机（Hades Canyon，Intel HD 630 VAAPI +
AMD Polaris 显示输出，官方 Mac 查看端）。/ Unit tests and the author's machine (Hades Canyon, Intel HD 630
VAAPI + AMD Polaris display, official Mac viewer).

**非官方项目，与网易无关。** / **Unofficial, not affiliated with NetEase.**
