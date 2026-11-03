# Builds the Windows release archive in target\release\package:
# Meeting-Transcriber-Windows-<arch>.zip, holding the program and a short
# README. The .exe is self-contained apart from the Visual C++ runtime.
#
#   .\scripts\package-windows.ps1              # Whisper on the processor
#   .\scripts\package-windows.ps1 -Vulkan      # plus a Vulkan build in vulkan\
#   .\scripts\package-windows.ps1 -CudaAddon   # only the CUDA add-on (-CUDA.zip)
#
# The program starts the best graphics-card build beside it that can run
# here (src/gpu.rs). The CUDA add-on is a zip of cuda\ (a CUDA build and
# CUDA's runtime DLLs), which the app downloads from the release of its
# version when it finds an NVIDIA card (src/cuda_addon.rs).
#
# -Vulkan needs the Vulkan SDK; -CudaAddon the CUDA toolkit (CUDA_PATH).
# Both need Ninja and a Developer PowerShell for Visual Studio.
param([switch]$Vulkan, [switch]$CudaAddon)
$ErrorActionPreference = 'Stop'

Set-Location (Join-Path $PSScriptRoot '..')

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x86_64' }
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$out = Join-Path $target 'release\package'
New-Item -ItemType Directory -Force -Path $out | Out-Null
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value

# A graphics-card build, in its own target folder, copied into $into\$feature.
function Add-GpuBuild([string]$feature, [string]$into) {
    # GPU_TARGET_DIR: a short folder for these builds. The Vulkan build's
    # shader tool nests deep enough to pass Windows' path limit under a
    # long checkout path.
    $root = if ($env:GPU_TARGET_DIR) { $env:GPU_TARGET_DIR } else { $target }
    $dir = Join-Path $root $feature
    # whisper.cpp's Vulkan build runs a CMake project of its own to make its
    # shaders, which under the Visual Studio generator finds no compiler.
    # Ninja, from a developer shell (cl.exe on the PATH), lets it find one.
    $generator = $env:CMAKE_GENERATOR
    if ((Get-Command ninja -ErrorAction SilentlyContinue) -and (Get-Command cl -ErrorAction SilentlyContinue)) {
        $env:CMAKE_GENERATOR = 'Ninja'
    }
    cargo build --release --features $feature --target-dir $dir
    $code = $LASTEXITCODE
    $env:CMAKE_GENERATOR = $generator
    if ($code -ne 0) { exit $code }
    $sub = Join-Path $into $feature
    New-Item -ItemType Directory -Force -Path $sub | Out-Null
    Copy-Item (Join-Path $dir 'release\meeting-transcriber.exe') $sub
    return $sub
}

if ($CudaAddon) {
    $name = "Meeting-Transcriber-Windows-$arch-CUDA"
    $stage = Join-Path $out $name
    $zip = Join-Path $out "$name.zip"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    if (Test-Path $zip) { Remove-Item -Force $zip }
    $sub = Add-GpuBuild 'cuda' $stage
    $dlls = Get-ChildItem (Join-Path $env:CUDA_PATH 'bin\*') -Include 'cudart64_*.dll', 'cublas64_*.dll', 'cublasLt64_*.dll'
    if (-not $dlls) { throw "no CUDA runtime DLLs in $env:CUDA_PATH\bin" }
    $dlls | Copy-Item -Destination $sub
    # The app uses the add-on only with the program of the same version.
    Set-Content -NoNewline -Encoding ascii -Path (Join-Path $sub 'version.txt') -Value $version
    # The zip holds cuda\ itself, as the app unpacks it.
    Compress-Archive -Path $sub -DestinationPath $zip
    Write-Host "Packaged: $zip"
    exit 0
}

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$name = "Meeting-Transcriber-Windows-$arch"
$stage = Join-Path $out $name
$zip = Join-Path $out "$name.zip"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
if (Test-Path $zip) { Remove-Item -Force $zip }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item (Join-Path $target 'release\meeting-transcriber.exe') $stage
if ($Vulkan) { Add-GpuBuild 'vulkan' $stage | Out-Null }
@'
Meeting Transcriber for Windows

Run meeting-transcriber.exe, and press Install in its window to put it in
the Start menu.

It needs the Microsoft Visual C++ Redistributable (x64), which most PCs
already have: https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist

Whisper runs on the graphics card when there is one: the vulkan folder holds
that build; keep it beside meeting-transcriber.exe. With an NVIDIA card, the
app offers to download CUDA support, which is faster still.

If Me stays silent, allow microphone access in Settings > Privacy & security >
Microphone, including "Let desktop apps access your microphone".

More: https://github.com/PatrickSun93/meeting-transcriber#windows
'@ | Set-Content -Encoding UTF8 (Join-Path $stage 'README.txt')

Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "Packaged: $zip"
