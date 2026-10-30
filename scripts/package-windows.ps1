# Builds the Windows release archive in target\release\package:
# Meeting-Transcriber-Windows-<arch>.zip, holding the program and a short
# README. The .exe is self-contained apart from the Visual C++ runtime.
#
#   .\scripts\package-windows.ps1                      # Whisper on the processor
#   .\scripts\package-windows.ps1 --features vulkan    # or on the graphics card
#
# Extra arguments go to `cargo build --release`.
$ErrorActionPreference = 'Stop'

Set-Location (Join-Path $PSScriptRoot '..')

cargo build --release @args
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x86_64' }
$name = "Meeting-Transcriber-Windows-$arch"
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$out = Join-Path $target 'release\package'
$stage = Join-Path $out $name
$zip = Join-Path $out "$name.zip"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
if (Test-Path $zip) { Remove-Item -Force $zip }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item (Join-Path $target 'release\meeting-transcriber.exe') $stage
@'
Meeting Transcriber for Windows

Run meeting-transcriber.exe. It is a single file: copy it anywhere and pin
it to Start or the taskbar.

It needs the Microsoft Visual C++ Redistributable (x64), which most PCs
already have: https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist

If Me stays silent, allow microphone access in Settings > Privacy & security >
Microphone, including "Let desktop apps access your microphone".

More: https://github.com/PatrickSun93/teams-meeting-plugin#windows
'@ | Set-Content -Encoding UTF8 (Join-Path $stage 'README.txt')

Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "Packaged: $zip"
