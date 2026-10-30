#!/usr/bin/env bash
# Installs Meeting Transcriber for the current user on Linux: the program in
# ~/.local/bin, a launcher in the app menu and its icon.
#
# From a checkout it builds first; extra arguments go to `cargo build --release`:
#   ./scripts/install-linux.sh                     # Whisper on the processor
#   ./scripts/install-linux.sh --features vulkan   # or on the graphics card
#
# The release tar.gz (scripts/package-linux.sh) ships this script as
# install.sh next to a ready-built program, which it installs as is.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
if [[ -x "$here/meeting-transcriber" && -f "$here/meeting-transcriber.desktop" ]]; then
    program="$here/meeting-transcriber"
    assets="$here"
else
    cd "$here/.."
    cargo build --release "$@"
    program="${CARGO_TARGET_DIR:-$PWD/target}/release/meeting-transcriber"
    assets="$PWD/assets"
fi

bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
apps_dir="$data_dir/applications"
icons_dir="$data_dir/icons/hicolor/scalable/apps"
mkdir -p "$bin_dir" "$apps_dir" "$icons_dir"
install -m 755 "$program" "$bin_dir/meeting-transcriber"
install -m 644 "$assets/meeting-transcriber.svg" "$icons_dir/meeting-transcriber.svg"
# The full path, since ~/.local/bin is not on every desktop session's PATH.
sed "s|^Exec=.*|Exec=\"$bin_dir/meeting-transcriber\"|" "$assets/meeting-transcriber.desktop" \
    > "$apps_dir/meeting-transcriber.desktop"
# Menus pick up the new icon without logging out (where the tools exist).
gtk-update-icon-cache -q -t "$data_dir/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$apps_dir" 2>/dev/null || true

echo "Installed $bin_dir/meeting-transcriber"
echo "Open Meeting Transcriber from the app menu, or run: meeting-transcriber"
if ! command -v parec >/dev/null 2>&1; then
    echo "Note: recording Others needs parec (package pulseaudio-utils; libpulse on Arch)."
fi
if [[ -n "${WAYLAND_DISPLAY:-}" ]] && ! command -v wtype >/dev/null 2>&1 && ! command -v ydotool >/dev/null 2>&1; then
    echo "Note: on Wayland, dictation pastes with wtype or ydotool; without them the text is only copied."
fi
if ! command -v zenity >/dev/null 2>&1 && ! command -v kdialog >/dev/null 2>&1; then
    echo "Note: without zenity or kdialog, folders are typed in instead of chosen in a dialog."
fi
