#!/usr/bin/env bash
# Builds Meeting Transcriber and installs it for the current user on Linux:
# the program in ~/.local/bin and a launcher in the app menu.
#
#   ./scripts/install-linux.sh                     # Whisper on the processor
#   ./scripts/install-linux.sh --features vulkan   # or on the graphics card
#
# Extra arguments go to `cargo build --release`.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --release "$@"

bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
apps_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$bin_dir" "$apps_dir"
install -m 755 target/release/meeting-transcriber "$bin_dir/meeting-transcriber"

# StartupWMClass matches the window's app id, so the launcher and the
# window share one icon in the dock.
cat > "$apps_dir/meeting-transcriber.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Meeting Transcriber
Comment=Live meeting transcription with a local Whisper model
Exec="$bin_dir/meeting-transcriber"
Icon=audio-input-microphone
Terminal=false
Categories=AudioVideo;Audio;Office;
StartupWMClass=meeting-transcriber
DESKTOP

echo "Installed $bin_dir/meeting-transcriber"
echo "Open Meeting Transcriber from the app menu, or run: meeting-transcriber"
if ! command -v parec >/dev/null 2>&1; then
    echo "Note: recording Others needs parec (package pulseaudio-utils; libpulse on Arch)."
fi
