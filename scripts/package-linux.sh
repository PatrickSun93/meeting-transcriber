#!/usr/bin/env bash
# Builds the Linux release archive in target/release/package:
# Meeting-Transcriber-Linux-<arch>.tar.gz, holding the program, its app-menu
# launcher and icon, and install.sh (scripts/install-linux.sh), which
# installs them for the current user.
#
#   ./scripts/package-linux.sh                   # Whisper on the processor
#   ./scripts/package-linux.sh --vulkan          # plus a Vulkan build in vulkan/
#   ./scripts/package-linux.sh --vulkan --cuda   # plus a CUDA build in cuda/ (-NVIDIA.tar.gz)
#
# The program starts the best graphics-card build beside it that can run
# here (src/gpu.rs). --vulkan needs the Vulkan headers and glslc; --cuda the
# CUDA toolkit (CUDA_PATH, or /usr/local/cuda), whose runtime libraries are
# copied in.
set -euo pipefail

vulkan=false
cuda=false
for arg in "$@"; do
    case "$arg" in
        --vulkan) vulkan=true ;;
        --cuda) cuda=true ;;
        *) echo "unknown option $arg" >&2; exit 1 ;;
    esac
done

cd "$(dirname "$0")/.."

if [[ "$(uname)" != "Linux" ]]; then
    echo "This script must run on Linux." >&2
    exit 1
fi

cargo build --release

target_dir="${CARGO_TARGET_DIR:-target}"
name="Meeting-Transcriber-Linux-$(uname -m)"
if $cuda; then name="$name-NVIDIA"; fi
out="$target_dir/release/package"
stage="$out/$name"
rm -rf "$stage" "$out/$name.tar.gz"
mkdir -p "$stage"
install -m 755 "$target_dir/release/meeting-transcriber" "$stage/meeting-transcriber"
install -m 755 scripts/install-linux.sh "$stage/install.sh"

# A graphics-card build in its own folder, built in its own target folder.
gpu_build() {
    cargo build --release --features "$1" --target-dir "$target_dir/$1"
    install -D -m 755 "$target_dir/$1/release/meeting-transcriber" "$stage/$1/meeting-transcriber"
}
if $vulkan; then gpu_build vulkan; fi
if $cuda; then
    gpu_build cuda
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
fi
install -m 644 assets/meeting-transcriber.desktop assets/meeting-transcriber.svg "$stage/"
cat > "$stage/README.txt" <<README
Meeting Transcriber for Linux

  ./install.sh             installs it for your user (~/.local/bin and the app menu)
  ./meeting-transcriber    runs it from here, without installing

Recording Others needs parec: package pulseaudio-utils (libpulse on Arch).
It works with PipeWire too. zenity or kdialog shows the folder dialog.
Whisper runs on the graphics card when there is one: the vulkan and cuda
folders (if this package has them) hold those builds, and the program picks
the one that runs here. Keep them beside the program.
More: https://github.com/PatrickSun93/meeting-transcriber#linux
README

tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "Packaged: $out/$name.tar.gz"
