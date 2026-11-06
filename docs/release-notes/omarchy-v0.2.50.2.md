「我的设备」上线：主窗口直接列出账号设备并一键远程其他电脑，控制端 `uuremote-viewer` 随包安装。/ The 我的设备 (my devices) card arrives: the main window lists the account's devices and connects out with one click; the `uuremote-viewer` controller ships in the package.

### 新增 / Added
- **我的设备卡片**：登录后主窗口显示账号设备列表（在线状态、预览缩略图），连接 / 观看 / 断开按钮直接发起远程桌面会话——每个会话是独立的 `uuremote-viewer` 进程，崩溃不影响本机被控服务；设备重启后会话继续可用。/ A device-list card (online state, preview thumbnails) with connect / watch / disconnect buttons. Each session is its own `uuremote-viewer` process, so a viewer crash never touches the host service; sessions survive a host restart.
- **控制端随包安装**：`uuremote-viewer`（SDL 远程桌面窗口，ffmpeg CLI 解码）进入 release tarball 与一键安装脚本（自动检查 sdl2 运行库）。/ The controller binary (SDL desktop window, ffmpeg CLI decode) joins the release tarball and the one-line installer (sdl2 runtime check included).

### 验证范围 / Validation
单元测试四套（342 / 347 / 527 / 657）；实机从 Omarchy 主机连接 MacBook 远程桌面验证。/ Four unit-test suites; live-verified connecting a MacBook desktop from the Omarchy host.

### 下载与安装 / Download
```sh
curl -fsSL https://raw.githubusercontent.com/fan-tastic-z/uu-remote-linux/omarchy/packaging/omarchy/install.sh | bash
```
或手动下载 `uuremote-omarchy-x86_64.tar.gz`（校验 `.sha256`），解压后运行 `./install.sh`。仅支持 Arch 系；GNOME/KDE/X11 请用上游版本。/ Or download `uuremote-omarchy-x86_64.tar.gz` manually (verify with `.sha256`), unpack and run `./install.sh`. Arch rolling only; for GNOME/KDE/X11 use the upstream release.

**非官方项目，与网易无关。** / **Unofficial, not affiliated with NetEase.**
