#!/usr/bin/env bash
# Builds the Linux release archive in target/release/package:
# Meeting-Transcriber-Linux-<arch>.tar.gz, holding the program, its app-menu
# launcher and icon, and install.sh (scripts/install-linux.sh), which
# installs them for the current user.
#
#   ./scripts/package-linux.sh                # Whisper on the processor
#   ./scripts/package-linux.sh --vulkan       # plus a Vulkan build in vulkan/
#   ./scripts/package-linux.sh --cuda-addon   # only the CUDA add-on (-CUDA.zip)
#
# The program starts the best graphics-card build beside it that can run
# here (src/gpu.rs). The CUDA add-on is a zip of cuda/ (a CUDA build and
# CUDA's runtime libraries), which the app downloads from the release of
# its version when it finds an NVIDIA card (src/cuda_addon.rs).
#
# --vulkan needs the Vulkan headers and glslc; --cuda-addon the CUDA
# toolkit (CUDA_PATH, or /usr/local/cuda) and python3 for the zip.
set -euo pipefail

vulkan=false
cuda_addon=false
for arg in "$@"; do
    case "$arg" in
        --vulkan) vulkan=true ;;
        --cuda-addon) cuda_addon=true ;;
        *) echo "unknown option $arg" >&2; exit 1 ;;
    esac
done

cd "$(dirname "$0")/.."

if [[ "$(uname)" != "Linux" ]]; then
    echo "This script must run on Linux." >&2
    exit 1
fi

target_dir="${CARGO_TARGET_DIR:-target}"
out="$target_dir/release/package"
mkdir -p "$out"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

# A graphics-card build, in its own target folder, copied into $2/$1.
gpu_build() {
    cargo build --release --features "$1" --target-dir "$target_dir/$1"
    install -D -m 755 "$target_dir/$1/release/meeting-transcriber" "$2/$1/meeting-transcriber"
}

if $cuda_addon; then
    name="Meeting-Transcriber-Linux-$(uname -m)-CUDA"
    stage="$out/$name"
    rm -rf "$stage" "$out/$name.zip"
    gpu_build cuda "$stage"
    cuda_dir="${CUDA_PATH:-/usr/local/cuda}"
    for lib in libcudart libcublas libcublasLt; do
        # The file by its soname (libcudart.so.12), whatever it links to.
        found=false
        for f in "$cuda_dir"/lib64/"$lib".so.*; do
            if [[ "$(basename "$f")" =~ ^$lib\.so\.[0-9]+$ ]]; then
                cp -L "$f" "$stage/cuda/"
                found=true
            fi
        done
        $found || { echo "no $lib in $cuda_dir/lib64" >&2; exit 1; }
    done
    # The app uses the add-on only with the program of the same version.
    printf '%s' "$version" > "$stage/cuda/version.txt"
    # A zip of cuda/ itself, as the app unpacks it, compressed; zipfile
    # keeps the program's executable bit.
    python3 - "$stage" "$out/$name.zip" <<'PY'
import os, sys, zipfile
stage, dest = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(dest, "w", zipfile.ZIP_DEFLATED) as z:
    for root, _, files in os.walk(os.path.join(stage, "cuda")):
        for f in sorted(files):
            path = os.path.join(root, f)
            z.write(path, os.path.relpath(path, stage))
PY
    echo "Packaged: $out/$name.zip"
    exit 0
fi

cargo build --release

name="Meeting-Transcriber-Linux-$(uname -m)"
stage="$out/$name"
rm -rf "$stage" "$out/$name.tar.gz"
mkdir -p "$stage"
install -m 755 "$target_dir/release/meeting-transcriber" "$stage/meeting-transcriber"
install -m 755 scripts/install-linux.sh "$stage/install.sh"
if $vulkan; then gpu_build vulkan "$stage"; fi
install -m 644 assets/meeting-transcriber.desktop assets/meeting-transcriber.svg "$stage/"
cat > "$stage/README.txt" <<README
Meeting Transcriber for Linux

  ./install.sh             installs it for your user (~/.local/bin and the app menu)
  ./meeting-transcriber    runs it from here, without installing

Recording Others needs parec: package pulseaudio-utils (libpulse on Arch).
It works with PipeWire too. zenity or kdialog shows the folder dialog.
Whisper runs on the graphics card when there is one: the vulkan folder holds
that build; keep it beside the program. With an NVIDIA card, the app offers
to download CUDA support, which is faster still.
More: https://github.com/PatrickSun93/meeting-transcriber#linux
README

tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "Packaged: $out/$name.tar.gz"
