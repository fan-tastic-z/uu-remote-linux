#!/bin/sh
# Install the omarchy build of uu-remote-linux for the current desktop user (Arch/Omarchy).
# Two modes:
#   ./install.sh                from the release tarball (uuremote-server sits next to this script)
#   curl -fsSL <repo>/install.sh | bash    downloads the latest release first, then installs
# usage: install.sh [--yes] [--no-ffmpeg] [--no-service]
#   --yes         install missing runtime packages (ffmpeg, sdl2) through sudo pacman without asking
#   --no-ffmpeg   only check runtime packages, never install them
#   --no-service  install files but do not enable/start the systemd user unit
set -eu

REPO=uu-remote-linux
OWNER=fan-tastic-z
ASSET=uuremote-omarchy-x86_64.tar.gz

assume_yes=0 ffmpeg_install=1 start_service=1
for arg in "$@"; do
  case $arg in
    -y|--yes) assume_yes=1 ;;
    --no-ffmpeg) ffmpeg_install=0 ;;
    --no-service) start_service=0 ;;
    -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $arg (see --help)" >&2; exit 2 ;;
  esac
done

if [ "$(id -u)" = 0 ]; then
  echo "Run this as your desktop user, not root: the host is a per-user service. It asks for sudo only for missing runtime packages." >&2
  exit 1
fi

src=$(cd "$(dirname "$0")" 2>/dev/null && pwd) || src=

# Bootstrap mode: piped from curl (or run outside the tarball) — fetch the latest release and
# hand over to its own copy of this script, where the binary sits next to it.
if [ ! -f "$src/uuremote-server" ]; then
  command -v curl >/dev/null || { echo "curl is required to download the release" >&2; exit 1; }
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  base="https://github.com/$OWNER/$REPO/releases/latest/download"
  echo "Downloading the latest omarchy release…"
  curl -fL --progress-bar -o "$tmp/$ASSET" "$base/$ASSET" -o "$tmp/$ASSET.sha256" "$base/$ASSET.sha256"
  (cd "$tmp" && sha256sum -c "$ASSET.sha256")
  tar xzf "$tmp/$ASSET" -C "$tmp"
  dir=$(tar tzf "$tmp/$ASSET" | head -1 | cut -d/ -f1)
  # No exec: the EXIT trap below must still clean $tmp up afterwards.
  sh "$tmp/$dir/install.sh" "$@"
  exit $?
fi

# The omarchy build targets Hyprland (Omarchy); other Wayland desktops should use upstream.
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
  echo "Note: no Wayland session detected. This build targets Omarchy (Arch + Hyprland); for GNOME/KDE/X11 use the upstream release instead." >&2
fi

# Runtime package check: FFmpeg drives desktop video (missing = 黑屏/正在传输画面) and SDL2
# the 我的设备 controller window; the rest (libva, mesa, libpulse) ships with any desktop.
# wl-clipboard only mirrors the clipboard.
missing=
command -v ffmpeg >/dev/null || missing="ffmpeg"
pacman -Q sdl2 >/dev/null 2>&1 || missing="${missing:+$missing }sdl2"
if [ -n "$missing" ] && [ "$ffmpeg_install" = 1 ]; then
  if [ "$assume_yes" = 1 ]; then ask=y
  else
    printf 'Missing runtime packages: %s. Install them with sudo pacman? [Y/n] ' "$missing"
    read -r ask || ask=y
  fi
  case $ask in
    n*|N*) echo "Skipped ($missing); desktop video and the device list's connect button need them." ;;
    *) sudo pacman -S --needed --noconfirm $missing ;;
  esac
fi
command -v wl-clipboard >/dev/null || echo "Tip: 'pacman -S wl-clipboard' enables the Wayland clipboard bridge."

bin=$HOME/.local/bin share=$HOME/.local/share
unit_dir=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user

# A running binary cannot be overwritten in place ("text file busy"): copy, then rename.
put() { install -Dm"$1" "$2" "$3.new" && mv -f "$3.new" "$3"; }

put 755 "$src/uuremote-server" "$bin/uuremote-server"
# The 我的设备 card spawns it for remote sessions; optional (the host works without).
[ -f "$src/uuremote-viewer" ] && put 755 "$src/uuremote-viewer" "$bin/uuremote-viewer"
if [ -d "$src/contrib/desktop" ]; then
  put 644 "$src/contrib/desktop/uu-remote-linux.svg" "$share/icons/hicolor/scalable/apps/uu-remote-linux.svg"
  put 644 "$src/contrib/desktop/uu-remote-linux.desktop" "$share/applications/uu-remote-linux.desktop"
fi
echo "Installed uuremote-server to $bin"

# Same unit as upstream's tarball, pointing at ~/.local/bin. An existing unit is the user's.
if [ -f "$unit_dir/uuremote-native.service" ]; then
  echo "Kept existing $unit_dir/uuremote-native.service"
else
  mkdir -p "$unit_dir"
  cat > "$unit_dir/uuremote-native.service" <<'UNIT'
[Unit]
Description=UU Remote Linux host (unofficial)
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Environment=RUST_LOG=info
ExecStart=%h/.local/bin/uuremote-server
Restart=always
RestartSec=5

[Install]
WantedBy=graphical-session.target
UNIT
fi

if [ "$start_service" = 1 ] && command -v systemctl >/dev/null; then
  systemctl --user daemon-reload
  if systemctl --user is-active -q uuremote-native.service; then
    echo "Restarting the service (this closes any active remote session)…"
    systemctl --user restart uuremote-native.service
  else
    systemctl --user enable --now uuremote-native.service
  fi
fi

echo
echo "Done. Next: run 'uuremote-server login' in a terminal inside your Hyprland session"
echo "and scan the QR code with the UU Remote mobile app. Logs: journalctl --user -u uuremote-native.service -f"
