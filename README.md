# Meeting Transcriber

A native app, written in Rust, that writes down your meetings as they happen. It is made for the Mac and also runs on [Windows and Linux](#windows-and-linux). Everything runs on your own computer, and no audio leaves it.

> [!WARNING]
> **Linux is untested; Windows is new.** The Mac build is the one in daily use. The Windows build has been installed and run on a real PC (the installer, the Start menu entry and uninstalling work), but it is much newer and less used, so recording, dictation, the tray and pasting may still have problems there. The Linux build compiles and passes its unit tests in CI, but nobody has run it on a Linux desktop yet. Please [open an issue](https://github.com/PatrickSun93/meeting-transcriber/issues) if something does not work.

- **Me**: whatever your microphone hears.
- **Others**: whatever the computer plays, so every other participant counts as one speaker.
- **Works with any meeting app**: Teams, Zoom, Meet, FaceTime and others. Nothing is installed inside the meeting app.
- **Any language**: speech is written in the language it was spoken. You can mix languages, and each line is detected on its own.
- **Near real time**: live text appears about a second after someone starts speaking and is finalised at each pause.
- **Dictation**: hold Right Option (Mac) or Right Ctrl (Windows, Linux) anywhere and speak; let go and the text is pasted where the cursor is. It uses the same Whisper model as meetings, loaded once. See [Dictation](#dictation).

Speech-to-text is [whisper.cpp](https://github.com/ggml-org/whisper.cpp) running on the Mac's GPU (Metal; on Windows and Linux the graphics card, with Vulkan or, on NVIDIA cards, CUDA, else the processor). It uses the `large-v3-turbo` model by default.

```
 microphone ──► Me ─────┐
                        ├─► chunk at pauses ─► Whisper (Metal GPU) ─► transcript window + file
 Mac audio ───► Others ─┘
 (ScreenCaptureKit)
```

## Install (Mac, Apple Silicon, macOS 13 or newer)

**Download:** get `Meeting-Transcriber-macOS.dmg` from the [latest release](https://github.com/PatrickSun93/meeting-transcriber/releases/latest), open it and drag **Meeting Transcriber** onto **Applications** (`Meeting-Transcriber-macOS.zip` holds the same app). It is not signed with an Apple Developer ID, so macOS blocks it the first time: open it once, then choose **Open Anyway** in **System Settings → Privacy & Security**. Then go on with step 4 below.

**Or build it yourself:**

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

4. Click **Download (547 MB)** to fetch the Whisper model. You only do this once. Other models can be downloaded from **Settings**. If the download fails, see [If a model download fails](#if-a-model-download-fails).

## First run: permissions

macOS asks for two permissions:

- **Microphone**: needed for *Me*. Click Allow when asked.
- **Screen & System Audio Recording**: needed for *Others*. The app only uses the audio. Allow it in **System Settings → Privacy & Security → Screen & System Audio Recording**, then **quit and reopen** the app.

If *Others* stays silent, this permission is almost always the reason. After you rebuild the app, macOS may treat it as a new app; switch the permission off and on again.

Dictation needs two more, both in **System Settings → Privacy & Security**:

- **Input Monitoring**: to see the hotkey from any app. Without it the app shows a banner with **Open System Settings** and **Try again**; everything else keeps working.
- **Accessibility**: to press ⌘V for you. Without it the text is still copied to the clipboard.

The app is signed ad hoc, so each rebuild looks like a new app to macOS: after `./scripts/bundle-macos.sh --install` (or `--dmg` for the disk image), switch Input Monitoring and Accessibility (like the other two) off and on again for **Meeting Transcriber**, then quit and reopen it.

## Command line and AI agents

Besides opening the window, the program answers on the command line, for
scripts, bug reports and AI agents such as Claude Code (see [AGENTS.md](AGENTS.md)):

| Command | What it does |
|---|---|
| `meeting-transcriber doctor [--json]` | What the app sees here: settings file, folders, models, hotkey, permissions, and what looks wrong |
| `meeting-transcriber gpu [install-cuda [--from URL_OR_ZIP]]` | Where Whisper runs and why; with an NVIDIA card, downloads (or unpacks) CUDA support |
| `meeting-transcriber log [--lines N]` | The end of the app's log (`meeting-transcriber.log` in the app data folder); on Windows, the only place errors go |
| `meeting-transcriber keys [--once] [--save] [--json]` | Prints each key or combination you hold and let go of, as the hotkey watcher sees it; `--save` makes it the dictation hotkey |
| `meeting-transcriber settings [path \| get KEY \| set KEY VALUE] [--json]` | Lists, reads or changes settings, checked before saving |
| `meeting-transcriber transcribe …` | Transcribes WAV files (see `meeting-transcriber help`) |

## Windows and Linux

> [!WARNING]
> Windows has been tried on a real PC, but much less than the Mac; Linux has only been compiled and unit-tested in CI, not run on a Linux desktop. Expect rough edges, and please report what you find.

The same app builds on Windows 10/11 and Linux, with the same features: *Me*, *Others*, the meeting library, notes, the reader and Settings. What differs:

| | macOS | Windows | Linux |
| --- | --- | --- | --- |
| *Others* is recorded from | ScreenCaptureKit | the default output device (WASAPI loopback) | the sound server's monitor of the default output (`parec`; PulseAudio or PipeWire) |
| Whisper runs on | the GPU (Metal) | the graphics card when there is one (Vulkan; on NVIDIA cards CUDA, downloaded when wanted), else the processor | the same as Windows |
| Settings and models | `~/Library/Application Support/Meeting Transcriber` | `%LOCALAPPDATA%\Meeting Transcriber` | `~/.local/share/Meeting Transcriber` |
| Folder dialog, file manager | Finder | Explorer | zenity or kdialog (else you type the path); the file manager over D-Bus, else `xdg-open` |
| Dictation hotkey (default) | Right Option, any app (Input Monitoring) | Right Ctrl, any app | Right Ctrl on X11; on Wayland the desktop's shortcut, or `meeting-transcriber dictate toggle` bound to one ([details](#linux-1)) |
| Dictation pastes with | ⌘V (Accessibility) | Ctrl+V | Ctrl+V: XTest on X11, `wtype` or `ydotool` on Wayland |
| Dictation lives in | the menu bar | the notification area (tray) | the tray, where the desktop has one (KDE, Cinnamon, XFCE…; GNOME with the AppIndicator extension) |

On the processor, **Large v3 Turbo** keeps up live only on a fast machine. If the status line shows more than "1× real time", pick **Small** or **Base** in Settings, or build with a GPU backend.

*Others* is whatever plays on the **default** output device. If the meeting app is set to a different speaker or headset than the system default, *Others* stays silent; make that device the default or switch the meeting app to it.

### Windows

**Download (new, see the note above):** from the [latest release](https://github.com/PatrickSun93/meeting-transcriber/releases/latest), run `Meeting-Transcriber-Setup-x86_64.exe`. It installs for your user, without administrator rights, into `%LOCALAPPDATA%\Programs\Meeting Transcriber`, with a Start menu shortcut (and a desktop one if you like), and uninstalls from **Settings → Apps → Installed apps**, which also removes a downloaded CUDA add-on; your settings, models and transcripts stay. Or get `Meeting-Transcriber-Windows-x86_64.zip`, unzip it and run `meeting-transcriber.exe`; keep the `vulkan` folder beside it. The program starts the graphics-card build that runs on this PC, and runs Whisper on the processor when none does (**Settings → Model** says which, and can turn it off; `meeting-transcriber gpu` too). With an NVIDIA card, **Settings → Model** offers **Download CUDA support** (about 600 MB, the `-CUDA.zip` of the same release, which brings its own CUDA libraries: no CUDA toolkit needed), which is faster still; **Restart now** puts it to use. `meeting-transcriber gpu install-cuda` does the same, or `--from` a downloaded zip. Run from the zip, it offers to **Install** itself the first time: it copies itself to `%LOCALAPPDATA%\Programs\Meeting Transcriber` (no administrator rights needed) and adds Meeting Transcriber to the Start menu and the desktop, so it is easy to find again; `meeting-transcriber install` does the same, `uninstall` takes the shortcuts away. Don't run it from inside the zip: Windows runs it from a temporary folder it later empties. Closing the window keeps it running in the notification area (perhaps under **^**). It is not code-signed, so the first time Windows SmartScreen may say "Windows protected your PC": click **More info → Run anyway**. It needs the [Microsoft Visual C++ Redistributable](https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist) (x64), which most PCs already have. Then go on with step 4 below.

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

For the GPU, install the [Vulkan SDK](https://vulkan.lunarg.com/) and build with `cargo build --release --features vulkan`, or the CUDA toolkit and `--features cuda` (NVIDIA). `.\scripts\package-windows.ps1 -Vulkan` packages the processor build with the Vulkan build in `vulkan\`, which it starts when it can; `-CudaAddon` builds only the CUDA add-on zip (`cuda\` with CUDA's DLLs). Both need Ninja and a Developer PowerShell for Visual Studio.

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

To hand it to someone else, `./scripts/package-linux.sh --vulkan` builds `target/release/package/Meeting-Transcriber-Linux-x86_64.tar.gz`: the program with a Vulkan build in `vulkan/`, its launcher and icon, and `install.sh`; `--cuda-addon` builds only the CUDA add-on zip (`cuda/` with CUDA's libraries), which the app downloads on PCs with an NVIDIA card. The program starts the graphics-card build that runs here, else Whisper runs on the processor. They unpack it and run `./install.sh` (or just `./meeting-transcriber`); they need `pulseaudio-utils` for *Others* but no build tools. CI builds the same archive (artifact *Meeting-Transcriber-Linux-x86_64*). It is built on Ubuntu, so it runs on distributions with the same or a newer glibc.

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

Hold a key, speak, and let go: the text is typed where the cursor is, in any app.

1. Hold **Right Option** on the Mac, **Right Ctrl** on Windows and Linux (the hotkey can be changed to any key or combination, such as Ctrl+Shift+D), in any app. You hear a short sound (*Tink* on the Mac).
2. Speak, then let go. The text is copied and pasted where the cursor is (*Pop*; *Basso* if something failed). Presses shorter than 0.35 s are ignored, and recording stops on its own after 5 minutes.
3. While a meeting is recording, what you say during dictation stays out of the meeting: *Me* is silent while the key is held.

Dictation is served before meeting audio by the same Whisper engine, so a meeting and dictation never load the model twice.

- **Menu bar** (Mac): a microphone icon shows the state (hourglass loading, mic ready, red mic recording, waveform transcribing, crossed-out mic paused, triangle after an error). Its menu has the last 10 dictations (click one to copy it), **Pause dictation**, **Open Meeting Transcriber** and **Quit**. Closing the window keeps the app in the menu bar, recording included; **Quit** (or ⌘Q) really quits.
- **History**: the sidebar's **Dictation** tab lists every recording by day, with its text; open one to read, play (in the system's audio player) or copy it. Recordings are WAV files (16 kHz mono) with a `.txt` of the same name. They go to `Dictation` in the app data folder (`~/Library/Application Support/Meeting Transcriber/Dictation` on the Mac; see [Windows and Linux](#windows-and-linux) for the others). The folder can be changed in Settings.
- **One copy dictates**: a second copy of Meeting Transcriber (one run from a build folder, say) stays paused while the first one dictates.
- **Clean-up** (off by default): a language model fixes punctuation and Chinese/English spacing. If it fails or is slow, the text is pasted as Whisper wrote it. **Settings → Dictation** picks the service:
  - **Ollama (this computer)**: a local model, `http://localhost:11434/api/generate` with `qwen2.5:3b` by default. Nothing leaves the computer.
  - **OpenAI-compatible API**: any service or local server that answers OpenAI's chat completions (`/v1/chat/completions`), such as OpenAI, DeepSeek, Qwen (DashScope), Groq, OpenRouter, LM Studio or vLLM. Set the URL, the model name the service uses, and the API key (sent as `Authorization: Bearer`; local servers may need none).
  - **Claude API (Anthropic)**: `https://api.anthropic.com/v1/messages` with `claude-opus-5-5` by default, at low effort. `claude-haiku-5-5` is much cheaper and faster, and enough for this job. Set the API key from the [Claude Console](https://platform.claude.com/).

  The API key can be left empty in Settings and set as `OPENAI_API_KEY` or `ANTHROPIC_API_KEY` in the environment instead; a key typed in Settings is saved in the settings file in plain text. With an internet API, the text of each dictation is sent to that service. Requests to the internet go through the proxy in `HTTPS_PROXY`, or on Windows the system proxy; ones to this computer do not.
- **Open at login** (Settings): on the Mac it adds `~/Library/LaunchAgents/com.patricksun.meeting-transcriber.plist`, which opens `/Applications/Meeting Transcriber.app` in the menu bar when you log in. Switching it off removes the file. Nothing is set up until you switch it on.

### Windows

- **Hotkey**: Right Ctrl by default, seen in every app through a low-level keyboard hook; no permission is needed. Windows drops such a hook without a word if it is ever slow to answer, so the app puts it back every 30 seconds. Right Alt is offered too, but on many keyboard layouts it is AltGr, which types characters. **Record keys…** takes any other key or combination (hold it, then let go); a combination's keys are kept from the app in front while it is held.
- **Pasting** presses Ctrl+V. Windows does not let an app type into one running as administrator, so in such a window the text is only copied: press Ctrl+V yourself.
- **Sounds** are the system's *Device Connect* (start), *Notification* (done) and *Critical Stop* (error), as set in **Settings → System → Sound → More sound settings**. Like everything the PC plays, they are part of a meeting's *Others* track while you record one.
- **Tray**: the icon in the notification area is a coloured microphone (green ready, red recording, amber transcribing, grey paused or loading, crimson `!` after an error). Left-click opens the window; right-click shows the last 10 dictations (click to copy), **Pause dictation**, **Open Meeting Transcriber** and **Quit**. Closing the window keeps the app there; Windows may tuck the icon under the **^** arrow, and **Settings → Personalization → Taskbar → Other system tray icons** keeps it in view.
- **Open at login** adds *Meeting Transcriber* to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, pointing at the `.exe` where it is now, started with `--hidden` (in the tray). It also appears in **Task Manager → Startup apps**. Move the `.exe` and switch it off and on again.

### Linux

How dictation hears the hotkey depends on the session:

- **X11**: Right Ctrl by default, seen in every app through XInput 2, like on the other systems; **Record keys…** takes any other key or combination (hold it, then let go); a combination's keys are kept from the app in front while it is held. Pasting presses Ctrl+V through XTest. Nothing needs to be installed or allowed.
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
| Dictation | On or off, **Paused**, the hotkey (Right Option, Command, Control or Shift on the Mac; Right Ctrl, Alt, Windows/Super or Shift elsewhere; F13–F20, or **Record keys…** for any other key or combination: hold it, then let go (on Windows a combination's keys are kept from the app in front while held); on Wayland the desktop's shortcut), language, an optional initial prompt (names, simplified Chinese, …), paste automatically, sounds, keep recordings and their folder, the shortest and longest clip, clean-up (service, URL, model, API key, timeout; see [Dictation](#dictation)), and **Open at login**. |

## Transcribe recordings

The same binary transcribes WAV files from the command line, with the same speaker labels and echo removal:

```bash
"/Applications/Meeting Transcriber.app/Contents/MacOS/meeting-transcriber" transcribe --me mic.wav --others meeting.wav
```

`--model` accepts a model id (`large-v3-turbo-q5_0`, `base`, …) or a path to a `ggml-*.bin` file. `--language` accepts a code such as `en` or `zh`.

On Linux, `meeting-transcriber dictate start|stop|toggle` starts and stops dictation in the running app (see [Linux](#linux-1) under Dictation).

## If a model download fails

Models come from `huggingface.co`. An error such as `No such host is known. (os error 11001)` (Windows) or `failed to lookup address` means this computer could not reach it, usually because of a VPN, a proxy or the network's DNS, even when a browser gets through.

- **Behind a VPN or proxy app:** on Windows the app uses the proxy set in the system's internet settings, the one the browser uses. It does not read a proxy auto-config (PAC) script. To pick a proxy yourself, start the app with `HTTPS_PROXY` set (any platform), using the address and port of your proxy:

  ```powershell
  $env:HTTPS_PROXY = "http://127.0.0.1:7890"
  & "$env:LOCALAPPDATA\Programs\Meeting Transcriber\meeting-transcriber.exe"
  ```

  On a VPN app with a TUN (virtual network adapter) mode, turning that on also works, as every program then goes through the VPN.
- **A company VPN:** its DNS may not answer for outside sites. Download while it is disconnected (a model is only downloaded once), or ask for the company proxy and set `HTTPS_PROXY` as above.
- **Download it in the browser:** open the link from the error message, which is `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/` followed by the model's file, and put the file, with its name unchanged, in the models folder (shown in **Settings**; by default the `models` folder in the app data folder, `%LOCALAPPDATA%\Meeting Transcriber\models` on Windows, see [Windows and Linux](#windows-and-linux)). The app finds it the next time it starts. Where `huggingface.co` is blocked, the same files are on mirrors such as `hf-mirror.com`, under the same path.

  | Model | File |
  |---|---|
  | Large v3 Turbo (recommended) | `ggml-large-v3-turbo-q5_0.bin` |
  | Large v3 Turbo, full precision | `ggml-large-v3-turbo.bin` |
  | Large v3 | `ggml-large-v3-q5_0.bin` |
  | Medium | `ggml-medium-q5_0.bin` |
  | Small | `ggml-small.bin` |
  | Base | `ggml-base.bin` |

`meeting-transcriber doctor` shows the models folder and which models it found.

## Development

```bash
cargo test        # unit tests: audio, chunking, echo filter, scheduling, settings, meeting files, dictation
cargo clippy --all-targets
cargo run --release
```

To publish a release, set `version` in `Cargo.toml`, commit, then push a tag with that version. CI builds and tests all three systems and attaches their packages, and the CUDA add-ons, to a new [GitHub release](https://github.com/PatrickSun93/meeting-transcriber/releases). The app downloads its CUDA add-on from the release of its own version, so the release job checks that the tag matches `Cargo.toml`. Push the tag, or publish a release with that tag on the Releases page (CI then adds the packages to it):

```bash
git tag v0.2.0
git push origin v0.2.0
```

To test without a meeting, these variables replace the live sources with WAV files played in real time:

```bash
MEETING_TRANSCRIBER_FAKE_ME=me.wav MEETING_TRANSCRIBER_FAKE_OTHERS=others.wav cargo run --release
```

`MEETING_TRANSCRIBER_MODELS` points at a different model folder and overrides the folder chosen in Settings. The default is the `models` folder in the app data folder (see [Windows and Linux](#windows-and-linux) for where that is).

CI builds and tests the app on an Apple Silicon runner, and attaches a zipped `.app` to each run. It also lints, tests and packages it on Windows and Linux, and attaches the Windows `.zip` and the Linux `.tar.gz`. A pull request runs nothing until it has the `ci` label (its checks show as skipped, which is not a pass); then it gets the processor builds only. The graphics-card builds (Vulkan, and the CUDA add-ons, which take the better part of an hour) are built on `main`, for version tags, when the workflow is run by hand (**Actions → CI → Run workflow**, on any branch), and for pull requests with the `gpu` label. Adding the label starts the run; for stacked pull requests, labelling the top one tests them all at once. `[skip ci]` in a commit message skips its run. sccache keeps compiled code between runs (on Windows only the Rust code).

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
| `src/dictation/` | Hold-to-talk dictation: recorder and finisher threads, hotkey names, the recordings archive, the language-model clean-up |
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
