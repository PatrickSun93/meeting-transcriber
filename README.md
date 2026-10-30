# Meeting Transcriber

A native app, written in Rust, that writes down your meetings as they happen. It is made for the Mac and also runs on [Windows and Linux](#windows-and-linux). Everything runs on your own computer, and no audio leaves it.

- **Me**: whatever your microphone hears.
- **Others**: whatever the computer plays, so every other participant counts as one speaker.
- **Works with any meeting app**: Teams, Zoom, Meet, FaceTime and others. Nothing is installed inside the meeting app.
- **Any language**: speech is written in the language it was spoken. You can mix languages, and each line is detected on its own.
- **Near real time**: live text appears about a second after someone starts speaking and is finalised at each pause.

Speech-to-text is [whisper.cpp](https://github.com/ggml-org/whisper.cpp) running on the Mac's GPU (Metal; on Windows and Linux the processor, or the graphics card with a Vulkan or CUDA build). It uses the `large-v3-turbo` model by default.

```
 microphone ──► Me ─────┐
                        ├─► chunk at pauses ─► Whisper (Metal GPU) ─► transcript window + file
 Mac audio ───► Others ─┘
 (ScreenCaptureKit)
```

## Install (Mac, Apple Silicon, macOS 13 or newer)

1. Install the build tools, once. You need the **Xcode 26** toolchain or newer, because the system-audio library uses the macOS 26 SDK. It installs on macOS 15.6 or later, and the app itself still runs on macOS 13+. Either install Xcode 26 from the App Store, or install the Command Line Tools and check that `swift --version` reports Swift 6.2 or newer:

   ```bash
   xcode-select --install
   ```

   ```bash
   brew install cmake
   ```

   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. Build the app and copy it to Applications:

   ```bash
   git clone https://github.com/PatrickSun93/teams-meeting-plugin.git
   cd teams-meeting-plugin
   ./scripts/bundle-macos.sh --install
   ```

   The first build compiles whisper.cpp and takes a few minutes.

3. Open **Meeting Transcriber** from Applications.

4. Click **Download (547 MB)** to fetch the Whisper model. You only do this once. Other models can be downloaded from **Settings**.

## First run: permissions

macOS asks for two permissions:

- **Microphone**: needed for *Me*. Click Allow when asked.
- **Screen & System Audio Recording**: needed for *Others*. The app only uses the audio. Allow it in **System Settings → Privacy & Security → Screen & System Audio Recording**, then **quit and reopen** the app.

If *Others* stays silent, this permission is almost always the reason. After you rebuild the app, macOS may treat it as a new app; switch the permission off and on again.

## Windows and Linux

The same app builds on Windows 10/11 and Linux, with the same features: *Me*, *Others*, the meeting library, notes, the reader and Settings. What differs:

| | macOS | Windows | Linux |
| --- | --- | --- | --- |
| *Others* is recorded from | ScreenCaptureKit | the default output device (WASAPI loopback) | the sound server's monitor of the default output (`parec`; PulseAudio or PipeWire) |
| Whisper runs on | the GPU (Metal) | the processor, or the GPU with `--features vulkan` / `cuda` | the same as Windows |
| Settings and models | `~/Library/Application Support/Meeting Transcriber` | `%LOCALAPPDATA%\Meeting Transcriber` | `~/.local/share/Meeting Transcriber` |
| Folder dialog, file manager | Finder | Explorer | zenity or kdialog (else you type the path); the file manager over D-Bus, else `xdg-open` |

On the processor, **Large v3 Turbo** keeps up live only on a fast machine. If the status line shows more than "1× real time", pick **Small** or **Base** in Settings, or build with a GPU backend.

*Others* is whatever plays on the **default** output device. If the meeting app is set to a different speaker or headset than the system default, *Others* stays silent; make that device the default or switch the meeting app to it.

### Windows

1. Install the build tools, once:
   - [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with **Desktop development with C++**
   - [Rust](https://rustup.rs) (the default MSVC toolchain)
   - CMake and LLVM (whisper.cpp's bindings need `libclang`) and Git:

     ```powershell
     winget install Kitware.CMake LLVM.LLVM Git.Git
     ```

2. Build it (the first build compiles whisper.cpp and takes a few minutes):

   ```powershell
   git clone https://github.com/PatrickSun93/teams-meeting-plugin.git
   cd teams-meeting-plugin
   cargo build --release
   ```

3. Run `target\release\meeting-transcriber.exe`. It is a single file: copy it anywhere and pin it to Start or the taskbar. On another PC it needs the [Microsoft Visual C++ Redistributable](https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist) (x64), which most PCs already have.

   To hand it to someone else, `.\scripts\package-windows.ps1` builds it and zips it with a short README into `target\release\package\Meeting-Transcriber-Windows-x86_64.zip` (extra arguments such as `--features vulkan` go to the build). CI does the same on every push (artifact *Meeting-Transcriber-Windows-x86_64*), so you can also download a ready-built zip from the Actions page and skip steps 1 and 2.

4. If *Me* stays silent, allow microphone access in **Settings → Privacy & security → Microphone**, including **Let desktop apps access your microphone**.

For the GPU, install the [Vulkan SDK](https://vulkan.lunarg.com/) and build with `cargo build --release --features vulkan`, or the CUDA toolkit and `--features cuda` (NVIDIA).

The command line works the same way: `meeting-transcriber.exe transcribe --me mic.wav`. A release build is a windowed program, so the terminal shows its prompt again before the transcript is done; in Command Prompt, `start /wait meeting-transcriber.exe transcribe --me mic.wav` waits for it.

### Linux

1. Install the build tools and libraries, once:

   ```bash
   # Debian, Ubuntu
   sudo apt install build-essential cmake clang libclang-dev libasound2-dev pulseaudio-utils zenity
   # Fedora
   sudo dnf install gcc-c++ cmake clang-devel alsa-lib-devel pulseaudio-utils zenity
   # Arch
   sudo pacman -S base-devel cmake clang alsa-lib libpulse zenity
   ```

   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

   `pulseaudio-utils` (`libpulse` on Arch) provides `parec`, which records *Others*; it works with PipeWire as well. `zenity` (or KDE's `kdialog`) shows the folder dialog; without either, the app asks you to type the folder's path instead.

2. Build and install it for your user (the program goes to `~/.local/bin`, a launcher to the app menu):

   ```bash
   git clone https://github.com/PatrickSun93/teams-meeting-plugin.git
   cd teams-meeting-plugin
   ./scripts/install-linux.sh
   ```

   Or just run it from the checkout with `cargo run --release`. For the GPU, add `--features vulkan` (needs the Vulkan headers and `glslc`, e.g. `libvulkan-dev glslc`) or `--features cuda`.

3. Open **Meeting Transcriber** from the app menu. It runs on X11 and Wayland.

To hand it to someone else, `./scripts/package-linux.sh` builds `target/release/package/Meeting-Transcriber-Linux-x86_64.tar.gz`: the program, its launcher and icon, and `install.sh`. They unpack it and run `./install.sh` (or just `./meeting-transcriber`); they need `pulseaudio-utils` for *Others* but no build tools. CI builds the same archive on every push (artifact *Meeting-Transcriber-Linux-x86_64*). It is built on Ubuntu, so it runs on distributions with the same or a newer glibc.

## Using it

1. Press the red record button before or during your meeting.
2. Talk. Grey italic text is the live guess. It becomes a normal line when the speaker pauses.
3. Press the button again (now a square) when you're done. Any remaining speech is still transcribed.

Every line is saved as you go (the folder can be changed in Settings). Type notes in the **Notes** panel next to the transcript while you record; the **📝 Notes** button hides or shows it.

## Meetings

Each meeting is one Markdown file in `~/Documents/Meeting Transcripts`, named `2026-10-29 14-05 <Title>.md`:

```markdown
---
title: "Weekly sync"
date: 2026-10-29T14:05
duration: 00:42:10
languages: [en, zh]
tags: [team, client]
---

# Weekly sync

## Notes

Your notes.

## Transcript

**[00:03] Me:** Hi, can you hear me?
**[00:07] Others:** Yes, loud and clear.
```

There is no AI summary: copy the meeting (**Copy all → As Markdown**) and summarize it with the assistant of your choice.

- **Sidebar**: **New meeting**, a search box (titles and tags; `#tag` searches tags only), and every meeting grouped by Today, Yesterday and date. Right-click a meeting to **Rename**, **Show in Finder** (Explorer, or the file manager) or **Delete** it. While recording, other meetings can't be opened.
- **Title and tags** are edited at the top of the page. Changing the title renames the file.
- **Autosave**: changes are written a second after you stop typing (and at least every few seconds), when recording stops, when you switch meetings and when you quit.
- **Continue a meeting**: open it and press record. The clock continues where it stopped.
- **Edit anywhere**: the files work in Obsidian or any editor. Extra frontmatter keys are kept, and the list picks up changes within a few seconds. An open meeting reloads when its file changes, unless you have unsaved edits in the app.
- **Reading a transcript**: show **All**, **Me** or **Others**; **Find in transcript** (⌘F; Ctrl+F on Windows and Linux) highlights matches, with Return / Shift-Return (or the arrows) for next and previous. Click a timestamp to copy that line, or right-click a line for more. Each meeting keeps its scroll position.
- **Export**: **Copy all** as Markdown or plain text; **Export** saves a `.txt` next to the meeting, or a `.txt` / `.md` to a folder you pick.
- **Old transcripts**: `Meeting <date> <time>.txt` files from earlier versions are listed (marked *txt*) and open read-only. **Convert to Markdown** makes a `.md` meeting from one and keeps the `.txt`.

**Headphones or speakers?** With headphones, your microphone only hears you. With speakers, it also hears the others. **Remove speaker echo** (on by default) drops your lines that repeat what Others said at the same moment. Short replies such as "Yes." are always kept. Headphones still give the cleanest result.

## Settings

Open **Settings** at the bottom of the sidebar. The app remembers your choices in `settings.txt` in `~/Library/Application Support/Meeting Transcriber` (Windows: `%LOCALAPPDATA%\Meeting Transcriber`, Linux: `~/.local/share/Meeting Transcriber`).

| Setting | Notes |
| --- | --- |
| Model | Every model with its size and whether it is downloaded: **Use**, **Download** (with Cancel), **Delete** and **Show in Finder** (Explorer, or the file manager). **Large v3 Turbo** is the best balance on M-series Macs. **Large v3** (1031 MB) is more accurate but slower. **Small** and **Base** are faster but less accurate. The status line shows "Whisper N× real time", and anything below 1 keeps up live. Models can't be switched while recording. |
| Models folder | Where models are stored. When you pick a new folder, the app offers to move the downloaded models there. If the app quits during the move, it finishes the move the next time it starts, and models not moved yet are still used from the old folder. Half-copied or half-downloaded files (`*.bin.part`) are deleted at startup. |
| Meetings folder | Where meetings are saved and listed from. |
| Language | **Auto-detect** handles mixed languages. Picking one language makes short phrases more reliable. |
| Me / Others | Turn off a source you don't need. |
| Remove speaker echo | See above. |

## Transcribe recordings

The same binary transcribes WAV files from the command line, with the same speaker labels and echo removal:

```bash
"/Applications/Meeting Transcriber.app/Contents/MacOS/meeting-transcriber" transcribe --me mic.wav --others meeting.wav
```

`--model` accepts a model id (`large-v3-turbo-q5_0`, `base`, …) or a path to a `ggml-*.bin` file. `--language` accepts a code such as `en` or `zh`.

## Development

```bash
cargo test        # unit tests: audio, chunking, echo filter, scheduling, settings, meeting files
cargo clippy --all-targets
cargo run --release
```

To test without a meeting, these variables replace the live sources with WAV files played in real time:

```bash
MEETING_TRANSCRIBER_FAKE_ME=me.wav MEETING_TRANSCRIBER_FAKE_OTHERS=others.wav cargo run --release
```

`MEETING_TRANSCRIBER_MODELS` points at a different model folder and overrides the folder chosen in Settings. The default is the `models` folder in the app data folder (see [Windows and Linux](#windows-and-linux) for where that is).

CI builds and tests the app on an Apple Silicon runner, and attaches a zipped `.app` to each run. It also lints, tests and packages it on Windows and Linux, and attaches the Windows `.zip` and the Linux `.tar.gz`.

| File | Role |
| --- | --- |
| `src/main.rs` | Starts the window, or the command line (`transcribe`, `--help`) |
| `src/audio/mic.rs` | Microphone capture with cpal; also Others on Windows (WASAPI loopback) |
| `src/audio/system/` | Others: ScreenCaptureKit on macOS, `parec` on Linux |
| `src/audio/silence.rs` | Fills gaps from loopback sources that go quiet, so timestamps stay right |
| `src/audio/file.rs` | WAV files played in real time in place of a live source, for testing |
| `src/audio/dsp.rs` | Low-pass filter and resampling to 16 kHz mono |
| `src/chunker.rs` | Splits speech at pauses; sends live partials |
| `src/session.rs` | Connects the sources, chunkers and transcriber |
| `src/transcriber.rs` | Whisper engine and job queue (finals first; stale partials dropped) |
| `src/transcript.rs` | Transcript, live previews, echo filter, hallucination filter |
| `src/app/mod.rs` | The window (egui): app state and background work |
| `src/app/meeting_page.rs` | Record button, title, tags, notes, footer actions |
| `src/app/reader.rs` | Transcript reader: filter, find, copy |
| `src/app/library.rs` | Open meeting: autosave, rename, delete, convert, export |
| `src/app/settings_page.rs` | Models, folders, recognition options |
| `src/app/path_prompt.rs` | Typing a folder's path when there is no folder dialog |
| `src/app/sidebar.rs` | New meeting, search, meeting list, Settings |
| `src/app/theme.rs` | Colours, spacing and fonts for dark and light mode |
| `src/platform/` | What differs per system: file manager, folder dialog, wording, console (macOS, Windows, Linux) |
| `src/models.rs` | Model list, folders, download and moving |
| `src/settings.rs` | Saved settings |
| `src/meeting.rs` | The Markdown meeting format (and old `.txt` transcripts) |
| `src/meetings.rs` | Meetings folder: file names, listing, search, date groups |
| `src/wav.rs` | Offline mode for WAV files |
| `scripts/` | macOS `.app` bundle, Linux install and `.tar.gz`, Windows `.zip` |
| `assets/` | Linux launcher (`.desktop`) and icon |

## License

MIT
