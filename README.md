# Meeting Transcriber

A native app, written in Rust, that writes down your meetings as they happen. It is made for the Mac and also runs on [Windows and Linux](#windows-and-linux). Everything runs on your own computer, and no audio leaves it.

> [!WARNING]
> **Windows and Linux are untested.** I use and test the app on the Mac only. The Windows and Linux builds compile and pass their unit tests in CI, but nobody has run them on a real PC yet, so recording, dictation, the tray and pasting may have problems there. Please [open an issue](https://github.com/PatrickSun93/meeting-transcriber/issues) if something does not work.

- **Me**: whatever your microphone hears.
- **Others**: whatever the computer plays, so every other participant counts as one speaker.
- **Works with any meeting app**: Teams, Zoom, Meet, FaceTime and others. Nothing is installed inside the meeting app.
- **Any language**: speech is written in the language it was spoken. You can mix languages, and each line is detected on its own.
- **Near real time**: live text appears about a second after someone starts speaking and is finalised at each pause.
- **Dictation**: hold Right Option (Mac) or Right Ctrl (Windows, Linux) anywhere and speak; let go and the text is pasted where the cursor is. It uses the same Whisper model as meetings, loaded once. See [Dictation](#dictation).

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
   git clone https://github.com/PatrickSun93/meeting-transcriber.git
   cd meeting-transcriber
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

Dictation needs two more, both in **System Settings → Privacy & Security**:

- **Input Monitoring**: to see the hotkey from any app. Without it the app shows a banner with **Open System Settings** and **Try again**; everything else keeps working.
- **Accessibility**: to press ⌘V for you. Without it the text is still copied to the clipboard.

The app is signed ad hoc, so each rebuild looks like a new app to macOS: after `./scripts/bundle-macos.sh --install`, switch Input Monitoring and Accessibility (like the other two) off and on again for **Meeting Transcriber**, then quit and reopen it.

## Windows and Linux

> [!WARNING]
> Not yet tested on a real Windows PC or Linux desktop: these builds have only been compiled and unit-tested in CI. Expect rough edges, and please report what you find.

The same app builds on Windows 10/11 and Linux, with the same features: *Me*, *Others*, the meeting library, notes, the reader and Settings. What differs:

| | macOS | Windows | Linux |
| --- | --- | --- | --- |
| *Others* is recorded from | ScreenCaptureKit | the default output device (WASAPI loopback) | the sound server's monitor of the default output (`parec`; PulseAudio or PipeWire) |
| Whisper runs on | the GPU (Metal) | the processor, or the GPU with `--features vulkan` / `cuda` | the same as Windows |
| Settings and models | `~/Library/Application Support/Meeting Transcriber` | `%LOCALAPPDATA%\Meeting Transcriber` | `~/.local/share/Meeting Transcriber` |
| Folder dialog, file manager | Finder | Explorer | zenity or kdialog (else you type the path); the file manager over D-Bus, else `xdg-open` |
| Dictation hotkey (default) | Right Option, any app (Input Monitoring) | Right Ctrl, any app | Right Ctrl on X11; on Wayland the desktop's shortcut, or `meeting-transcriber dictate toggle` bound to one ([details](#linux-1)) |
| Dictation pastes with | ⌘V (Accessibility) | Ctrl+V | Ctrl+V: XTest on X11, `wtype` or `ydotool` on Wayland |
| Dictation lives in | the menu bar | the notification area (tray) | the tray, where the desktop has one (KDE, Cinnamon, XFCE…; GNOME with the AppIndicator extension) |

On the processor, **Large v3 Turbo** keeps up live only on a fast machine. If the status line shows more than "1× real time", pick **Small** or **Base** in Settings, or build with a GPU backend.

*Others* is whatever plays on the **default** output device. If the meeting app is set to a different speaker or headset than the system default, *Others* stays silent; make that device the default or switch the meeting app to it.

### Windows

**Download (untested on a real PC, see the warning above):** get `Meeting-Transcriber-Windows-x86_64.zip` from the [latest release](https://github.com/PatrickSun93/meeting-transcriber/releases/latest), unzip it and run `meeting-transcriber.exe`. It is a single file: copy it anywhere and pin it to Start or the taskbar. It is not code-signed, so the first time Windows SmartScreen may say "Windows protected your PC": click **More info → Run anyway**. It needs the [Microsoft Visual C++ Redistributable](https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist) (x64), which most PCs already have. Then go on with step 4 below.

To build it yourself instead:

1. Install the build tools, once:
   - [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with **Desktop development with C++**
   - [Rust](https://rustup.rs) (the default MSVC toolchain)
   - CMake and LLVM (whisper.cpp's bindings need `libclang`) and Git:

     ```powershell
     winget install Kitware.CMake LLVM.LLVM Git.Git
     ```

2. Build it (the first build compiles whisper.cpp and takes a few minutes):

   ```powershell
   git clone https://github.com/PatrickSun93/meeting-transcriber.git
   cd meeting-transcriber
   cargo build --release
   ```

3. Run `target\release\meeting-transcriber.exe`. It is a single file: copy it anywhere and pin it to Start or the taskbar. On another PC it needs the [Microsoft Visual C++ Redistributable](https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist) (x64), which most PCs already have.

   To hand it to someone else, `.\scripts\package-windows.ps1` builds it and zips it with a short README into `target\release\package\Meeting-Transcriber-Windows-x86_64.zip` (extra arguments such as `--features vulkan` go to the build). CI does the same on every push (artifact *Meeting-Transcriber-Windows-x86_64*).

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

   `pulseaudio-utils` (`libpulse` on Arch) provides `parec`, which records *Others*; it works with PipeWire as well. `zenity` (or KDE's `kdialog`) shows the folder dialog; without either, the app asks you to type the folder's path instead. On Wayland, dictation pastes with `wtype` or `ydotool` (see [Dictation on Linux](#linux-1)); install one of them too.

2. Build and install it for your user (the program goes to `~/.local/bin`, a launcher to the app menu):

   ```bash
   git clone https://github.com/PatrickSun93/meeting-transcriber.git
   cd meeting-transcriber
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

## Dictation

This replaces the old voice2cursor tool and works the same way:

1. Hold **Right Option** on the Mac, **Right Ctrl** on Windows and Linux (the hotkey can be changed), in any app. You hear a short sound (*Tink* on the Mac).
2. Speak, then let go. The text is copied and pasted where the cursor is (*Pop*; *Basso* if something failed). Presses shorter than 0.35 s are ignored, and recording stops on its own after 5 minutes.
3. While a meeting is recording, what you say during dictation stays out of the meeting: *Me* is silent while the key is held.

Dictation is served before meeting audio by the same Whisper engine, so a meeting and dictation never load the model twice.

- **Menu bar** (Mac): a microphone icon shows the state (hourglass loading, mic ready, red mic recording, waveform transcribing, crossed-out mic paused, triangle after an error). Its menu has the last 10 dictations (click one to copy it), **Pause dictation**, **Open Meeting Transcriber** and **Quit**. Closing the window keeps the app in the menu bar, recording included; **Quit** (or ⌘Q) really quits.
- **History**: the sidebar's **Dictation** tab lists every recording by day, with its text; open one to read, play (in the system's audio player) or copy it. Recordings are WAV files (16 kHz mono) with a `.txt` of the same name. On the Mac they go to `~/.voice2cursor/recordings`, voice2cursor's folder, so its earlier recordings are listed too; on Windows and Linux to `Dictation` in the app data folder (see [Windows and Linux](#windows-and-linux)). The folder can be changed in Settings.
- **voice2cursor** (Mac): if it is running (it listens for the same key), dictation pauses and says so, also when it starts later. Stop it with `/Volumes/externalssd/devitems/tools/voice2cursor/stop.sh`, then press **Resume dictation**. A second copy of Meeting Transcriber (one run from a build folder, say) also stays paused while the first one dictates.
- **Ollama** (off by default): a local model fixes punctuation and Chinese/English spacing, with voice2cursor's prompt. If Ollama fails or is slow, the text is pasted as Whisper wrote it.
- **Moving over from voice2cursor** (Mac):
  1. Stop it: `/Volumes/externalssd/devitems/tools/voice2cursor/stop.sh`. This only stops it until the next login.
  2. In Meeting Transcriber, press **Resume dictation** if it paused, and dictate for a while. Its earlier recordings are already in the **Dictation** tab.
  3. Once you are happy, remove it for good: `/Volumes/externalssd/devitems/tools/voice2cursor/uninstall.sh`. It removes its login item and state but keeps `~/.voice2cursor/recordings`, which Meeting Transcriber keeps using. Then switch on **Open at login** here.
- **Open at login** (Settings): on the Mac it adds `~/Library/LaunchAgents/com.patricksun.meeting-transcriber.plist`, which opens `/Applications/Meeting Transcriber.app` in the menu bar when you log in. Switching it off removes the file. Nothing is set up until you switch it on.

### Windows

- **Hotkey**: Right Ctrl by default, seen in every app through a low-level keyboard hook; no permission is needed. Windows drops such a hook without a word if it is ever slow to answer, so the app puts it back every 30 seconds. Right Alt is offered too, but on many keyboard layouts it is AltGr, which types characters. **Press a key…** takes any other key.
- **Pasting** presses Ctrl+V. Windows does not let an app type into one running as administrator, so in such a window the text is only copied: press Ctrl+V yourself.
- **Sounds** are the system's *Device Connect* (start), *Notification* (done) and *Critical Stop* (error), as set in **Settings → System → Sound → More sound settings**. Like everything the PC plays, they are part of a meeting's *Others* track while you record one.
- **Tray**: the icon in the notification area is a coloured microphone (green ready, red recording, amber transcribing, grey paused or loading, crimson `!` after an error). Left-click opens the window; right-click shows the last 10 dictations (click to copy), **Pause dictation**, **Open Meeting Transcriber** and **Quit**. Closing the window keeps the app there; Windows may tuck the icon under the **^** arrow, and **Settings → Personalization → Taskbar → Other system tray icons** keeps it in view.
- **Open at login** adds *Meeting Transcriber* to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, pointing at the `.exe` where it is now, started with `--hidden` (in the tray). It also appears in **Task Manager → Startup apps**. Move the `.exe` and switch it off and on again.

### Linux

How dictation hears the hotkey depends on the session:

- **X11**: Right Ctrl by default, seen in every app through XInput 2, like on the other systems; **Press a key…** takes any other key. Pasting presses Ctrl+V through XTest. Nothing needs to be installed or allowed.
- **Wayland**: apps may not watch the keyboard, so the app asks the desktop for a global shortcut through the [GlobalShortcuts portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html) (KDE Plasma 5.27+, GNOME 48+, Hyprland). The desktop asks you once which keys to use; holding them records and letting go transcribes. The Dictation settings show the keys, and the desktop's shortcut settings change them. Pasting on Wayland needs [`wtype`](https://github.com/atx/wtype) (KDE, Sway, Hyprland) or [`ydotool`](https://github.com/ReimuNotMoe/ydotool) with its `ydotoold` service running (GNOME, which wtype can't type into; the older 0.1 versions in Debian 12 and Ubuntu 22.04 work too); without either, the text is only copied, which the app says once.
- **Any session, any key**: the running app also takes commands, so any shortcut can dictate:

  ```bash
  meeting-transcriber dictate toggle   # start, or stop and transcribe
  meeting-transcriber dictate start    # as if the hotkey went down…
  meeting-transcriber dictate stop     # …and came up
  ```

  Desktop shortcuts usually fire once per press, so bind **toggle**: press once to start, again to stop. A toggle goes by what dictation is doing, so after a pause or a clip that hit the 5-minute limit the next press starts again. Use the full path if `~/.local/bin` is not on the desktop's PATH (`./scripts/install-linux.sh` installs it there).

  - **GNOME**: **Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts → +**. Name *Dictate*, command `meeting-transcriber dictate toggle`, then press the shortcut (e.g. Super+D).
  - **KDE Plasma**: **System Settings → Keyboard → Shortcuts → Add New → Command or Script…**, command `meeting-transcriber dictate toggle`, then set the shortcut.
  - Sway and i3: `bindsym $mod+d exec meeting-transcriber dictate toggle`. They can also run a command when a key is let go, which gives hold-to-talk: `bindsym $mod+d exec meeting-transcriber dictate start` and `bindsym --release $mod+d exec meeting-transcriber dictate stop`. Hyprland: `bind` and `bindr` the same way.

  The commands go over a socket only you can open, `$XDG_RUNTIME_DIR/meeting-transcriber.sock`.

Also on Linux:

- **Sounds** come from the freedesktop sound theme (*audio-volume-change*, *complete*, *dialog-warning*), through `canberra-gtk-play`, else `paplay` or `pw-play`; with none of them there are no sounds. They are part of a meeting's *Others* track while you record one.
- **Tray**: a StatusNotifierItem, the same coloured microphone as on Windows, with the same menu; clicking it opens the window. KDE, Cinnamon, XFCE, LXQt and Budgie show it; GNOME only with the AppIndicator extension (Ubuntu has it on). Without a tray, closing the window quits the app, dictation included. So it does on Wayland, where no app may hide its window: minimize it instead to keep dictating.
- **Open at login** writes `~/.config/autostart/meeting-transcriber.desktop`, which starts the program where it is now with `--hidden` (in the tray, where there is one; on Wayland with the window open). Switching it off removes the file.

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
| Dictation | On or off, **Paused**, the hotkey (Right Option, Command, Control or Shift on the Mac; Right Ctrl, Alt, Windows/Super or Shift elsewhere; F13–F20, or **Press a key…** for any other; on Wayland the desktop's shortcut), language, an optional initial prompt (names, simplified Chinese, …), paste automatically, sounds, keep recordings and their folder, the shortest and longest clip, Ollama clean-up (URL, model, timeout), and **Open at login**. |

## Transcribe recordings

The same binary transcribes WAV files from the command line, with the same speaker labels and echo removal:

```bash
"/Applications/Meeting Transcriber.app/Contents/MacOS/meeting-transcriber" transcribe --me mic.wav --others meeting.wav
```

`--model` accepts a model id (`large-v3-turbo-q5_0`, `base`, …) or a path to a `ggml-*.bin` file. `--language` accepts a code such as `en` or `zh`.

On Linux, `meeting-transcriber dictate start|stop|toggle` starts and stops dictation in the running app (see [Linux](#linux-1) under Dictation).

## Development

```bash
cargo test        # unit tests: audio, chunking, echo filter, scheduling, settings, meeting files, dictation
cargo clippy --all-targets
cargo run --release
```

To publish a release, set `version` in `Cargo.toml`, commit, then push a tag with that version. CI builds and tests all three systems and attaches their packages to a new [GitHub release](https://github.com/PatrickSun93/meeting-transcriber/releases):

```bash
git tag v0.2.0
git push origin v0.2.0
```

To test without a meeting, these variables replace the live sources with WAV files played in real time:

```bash
MEETING_TRANSCRIBER_FAKE_ME=me.wav MEETING_TRANSCRIBER_FAKE_OTHERS=others.wav cargo run --release
```

`MEETING_TRANSCRIBER_MODELS` points at a different model folder and overrides the folder chosen in Settings. The default is the `models` folder in the app data folder (see [Windows and Linux](#windows-and-linux) for where that is).

CI builds and tests the app on an Apple Silicon runner, and attaches a zipped `.app` to each run. It also lints, tests and packages it on Windows and Linux, and attaches the Windows `.zip` and the Linux `.tar.gz`.

| File | Role |
| --- | --- |
| `src/main.rs` | Starts the window, or the command line (`transcribe`, `dictate`, `--help`) |
| `src/audio/mic.rs` | Microphone capture with cpal; also Others on Windows (WASAPI loopback) |
| `src/audio/system/` | Others: ScreenCaptureKit on macOS, `parec` on Linux |
| `src/audio/silence.rs` | Fills gaps from loopback sources that go quiet, so timestamps stay right |
| `src/audio/file.rs` | WAV files played in real time in place of a live source, for testing |
| `src/audio/dsp.rs` | Low-pass filter and resampling to 16 kHz mono |
| `src/chunker.rs` | Splits speech at pauses; sends live partials |
| `src/session.rs` | Connects the sources, chunkers and transcriber; silences Me while dictating |
| `src/transcriber.rs` | Whisper engine and job queue (dictation first, then finals; stale partials dropped) |
| `src/dictation/` | Hold-to-talk dictation: recorder and finisher threads, hotkey names, the recordings archive, Ollama clean-up, noticing voice2cursor |
| `src/transcript.rs` | Transcript, live previews, echo filter, hallucination filter |
| `src/app/mod.rs` | The window (egui): app state and background work |
| `src/app/meeting_page.rs` | Record button, title, tags, notes, footer actions |
| `src/app/reader.rs` | Transcript reader: filter, find, copy |
| `src/app/library.rs` | Open meeting: autosave, rename, delete, convert, export |
| `src/app/settings_page.rs` | Models, folders, recognition options |
| `src/app/dictation.rs` | Dictation in the window: hotkey, engine hand-over, menu bar item, closing to the menu bar |
| `src/app/dictation_page.rs` | Dictation banners, history in the sidebar, the Dictation page and its Settings card |
| `src/app/path_prompt.rs` | Typing a folder's path when there is no folder dialog |
| `src/app/sidebar.rs` | Meetings / Dictation, New meeting, search, the lists, Settings |
| `src/app/theme.rs` | Colours, spacing and fonts for dark and light mode |
| `src/platform/` | What differs per system: file manager, folder dialog, wording, console; for dictation the hotkey, pasting, sounds, the menu bar or tray item and the login item. `macos/`: Quartz event tap, ⌘V, NSStatusItem, launchd. `windows/`: keyboard hook, SendInput, Shell_NotifyIcon, the Run key. `linux/`: XInput 2 / XTest, the shortcuts portal, the `dictate` socket, a StatusNotifierItem, autostart. `shared.rs`: their plain logic (tray menu and icon, login entries, socket commands), tested on every system |
| `src/models.rs` | Model list, folders, download and moving |
| `src/settings.rs` | Saved settings |
| `src/meeting.rs` | The Markdown meeting format (and old `.txt` transcripts) |
| `src/meetings.rs` | Meetings folder: file names, listing, search, date groups |
| `src/wav.rs` | Offline mode for WAV files |
| `scripts/` | macOS `.app` bundle, Linux install and `.tar.gz`, Windows `.zip` |
| `assets/` | Linux launcher (`.desktop`) and icon |

## License

MIT
