# Builds the Windows release archive in target\release\package:
# Meeting-Transcriber-Windows-<arch>.zip, holding the program and a short
# README. The .exe is self-contained apart from the Visual C++ runtime.
#
#   .\scripts\package-windows.ps1                  # Whisper on the processor
#   .\scripts\package-windows.ps1 -Vulkan          # plus a Vulkan build in vulkan\
#   .\scripts\package-windows.ps1 -Vulkan -Cuda    # plus a CUDA build in cuda\ (-NVIDIA.zip)
#
# The program starts the best graphics-card build beside it that can run
# here (src/gpu.rs). -Vulkan needs the Vulkan SDK, Ninja, and a Developer
# PowerShell for Visual Studio; -Cuda the CUDA toolkit (CUDA_PATH), whose
# runtime DLLs are copied in.
param([switch]$Vulkan, [switch]$Cuda)
$ErrorActionPreference = 'Stop'

Set-Location (Join-Path $PSScriptRoot '..')

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x86_64' }
$name = "Meeting-Transcriber-Windows-$arch"
if ($Cuda) { $name += '-NVIDIA' }
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$out = Join-Path $target 'release\package'
$stage = Join-Path $out $name
$zip = Join-Path $out "$name.zip"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
if (Test-Path $zip) { Remove-Item -Force $zip }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item (Join-Path $target 'release\meeting-transcriber.exe') $stage

# A graphics-card build in its own folder, built in its own target folder.
function Add-GpuBuild([string]$feature) {
    # GPU_TARGET_DIR: a short folder for these builds. The Vulkan build's
    # shader tool nests deep enough to pass Windows' path limit under a
    # long checkout path.
    $root = if ($env:GPU_TARGET_DIR) { $env:GPU_TARGET_DIR } else { $target }
    $dir = Join-Path $root $feature
    # whisper.cpp's Vulkan build runs a CMake project of its own to make its
    # shaders, which under the Visual Studio generator finds no compiler.
    # Ninja, from a developer shell (cl.exe on the PATH), lets it find one.
    $generator = $env:CMAKE_GENERATOR
    if ($feature -eq 'vulkan' -and (Get-Command ninja -ErrorAction SilentlyContinue) -and
        (Get-Command cl -ErrorAction SilentlyContinue)) {
        $env:CMAKE_GENERATOR = 'Ninja'
    }
    cargo build --release --features $feature --target-dir $dir
    $code = $LASTEXITCODE
    $env:CMAKE_GENERATOR = $generator
    if ($code -ne 0) { exit $code }
    $sub = Join-Path $stage $feature
    New-Item -ItemType Directory -Force -Path $sub | Out-Null
    Copy-Item (Join-Path $dir 'release\meeting-transcriber.exe') $sub
    return $sub
}
if ($Vulkan) { Add-GpuBuild 'vulkan' | Out-Null }
if ($Cuda) {
    $sub = Add-GpuBuild 'cuda'
    $dlls = Get-ChildItem (Join-Path $env:CUDA_PATH 'bin\*') -Include 'cudart64_*.dll', 'cublas64_*.dll', 'cublasLt64_*.dll'
    if (-not $dlls) { throw "no CUDA runtime DLLs in $env:CUDA_PATH\bin" }
    $dlls | Copy-Item -Destination $sub
}
@'
Meeting Transcriber for Windows

Run meeting-transcriber.exe. It is a single file: copy it anywhere and pin
it to Start or the taskbar.

It needs the Microsoft Visual C++ Redistributable (x64), which most PCs
already have: https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist

Whisper runs on the graphics card when there is one: the vulkan and cuda
folders (if this package has them) hold those builds, and the program picks
the one that runs here. Keep them beside meeting-transcriber.exe.

If Me stays silent, allow microphone access in Settings > Privacy & security >
Microphone, including "Let desktop apps access your microphone".

More: https://github.com/PatrickSun93/meeting-transcriber#windows
'@ | Set-Content -Encoding UTF8 (Join-Path $stage 'README.txt')

Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "Packaged: $zip"
