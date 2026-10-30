#!/usr/bin/env bash
# Builds the Linux release archive in target/release/package:
# Meeting-Transcriber-Linux-<arch>.tar.gz, holding the program, its app-menu
# launcher and icon, and install.sh (scripts/install-linux.sh), which
# installs them for the current user.
#
#   ./scripts/package-linux.sh                     # Whisper on the processor
#   ./scripts/package-linux.sh --features vulkan   # or on the graphics card
#
# Extra arguments go to `cargo build --release`.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ "$(uname)" != "Linux" ]]; then
    echo "This script must run on Linux." >&2
    exit 1
fi

cargo build --release "$@"

target_dir="${CARGO_TARGET_DIR:-target}"
name="Meeting-Transcriber-Linux-$(uname -m)"
out="$target_dir/release/package"
stage="$out/$name"
rm -rf "$stage" "$out/$name.tar.gz"
mkdir -p "$stage"
install -m 755 "$target_dir/release/meeting-transcriber" "$stage/meeting-transcriber"
install -m 755 scripts/install-linux.sh "$stage/install.sh"
install -m 644 assets/meeting-transcriber.desktop assets/meeting-transcriber.svg "$stage/"
cat > "$stage/README.txt" <<README
Meeting Transcriber for Linux

  ./install.sh             installs it for your user (~/.local/bin and the app menu)
  ./meeting-transcriber    runs it from here, without installing

Recording Others needs parec: package pulseaudio-utils (libpulse on Arch).
It works with PipeWire too. zenity or kdialog shows the folder dialog.
More: https://github.com/PatrickSun93/meeting-transcriber#linux
README

tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "Packaged: $out/$name.tar.gz"
