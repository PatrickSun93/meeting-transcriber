# Meeting Transcriber

A native Mac app, written in Rust, that writes down your meetings as they happen. Everything runs on your own Mac, and no audio leaves it.

- **Me**: whatever your microphone hears.
- **Others**: whatever the Mac plays, so every other participant counts as one speaker.
- **Works with any meeting app**: Teams, Zoom, Meet, FaceTime and others. Nothing is installed inside the meeting app.
- **Any language**: speech is written in the language it was spoken. You can mix languages, and each line is detected on its own.
- **Near real time**: live text appears about a second after someone starts speaking and is finalised at each pause.

Speech-to-text is [whisper.cpp](https://github.com/ggml-org/whisper.cpp) running on the Mac's GPU (Metal). It uses the `large-v3-turbo` model by default.

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

- **Sidebar**: **New meeting**, a search box (titles and tags; `#tag` searches tags only), and every meeting grouped by Today, Yesterday and date. Right-click a meeting to **Rename**, **Show in Finder** or **Delete** it. While recording, other meetings can't be opened.
- **Title and tags** are edited at the top of the page. Changing the title renames the file.
- **Autosave**: changes are written a second after you stop typing (and at least every few seconds), when recording stops, when you switch meetings and when you quit.
- **Continue a meeting**: open it and press record. The clock continues where it stopped.
- **Edit anywhere**: the files work in Obsidian or any editor. Extra frontmatter keys are kept, and the list picks up changes within a few seconds. An open meeting reloads when its file changes, unless you have unsaved edits in the app.
- **Reading a transcript**: show **All**, **Me** or **Others**; **Find in transcript** (⌘F) highlights matches, with Return / Shift-Return (or the arrows) for next and previous. Click a timestamp to copy that line, or right-click a line for more. Each meeting keeps its scroll position.
- **Export**: **Copy all** as Markdown or plain text; **Export** saves a `.txt` next to the meeting, or a `.txt` / `.md` to a folder you pick.
- **Old transcripts**: `Meeting <date> <time>.txt` files from earlier versions are listed (marked *txt*) and open read-only. **Convert to Markdown** makes a `.md` meeting from one and keeps the `.txt`.

**Headphones or speakers?** With headphones, your microphone only hears you. With speakers, it also hears the others. **Remove speaker echo** (on by default) drops your lines that repeat what Others said at the same moment. Short replies such as "Yes." are always kept. Headphones still give the cleanest result.

## Settings

Open **Settings** at the bottom of the sidebar. The app remembers your choices in `~/Library/Application Support/Meeting Transcriber/settings.txt`.

| Setting | Notes |
| --- | --- |
| Model | Every model with its size and whether it is downloaded: **Use**, **Download** (with Cancel), **Delete** and **Show in Finder**. **Large v3 Turbo** is the best balance on M-series Macs. **Large v3** (1031 MB) is more accurate but slower. **Small** and **Base** are faster but less accurate. The status line shows "Whisper N× real time", and anything below 1 keeps up live. Models can't be switched while recording. |
| Models folder | Where models are stored. When you pick a new folder, the app offers to move the downloaded models there. |
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

`MEETING_TRANSCRIBER_MODELS` points at a different model folder and overrides the folder chosen in Settings. The default is `~/Library/Application Support/Meeting Transcriber/models`.

CI builds and tests the app on an Apple Silicon runner, and attaches a zipped `.app` to each run.

| File | Role |
| --- | --- |
| `src/audio/mic.rs` | Microphone capture (cpal / Core Audio) |
| `src/audio/system.rs` | Mac audio capture (ScreenCaptureKit) |
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
| `src/app/sidebar.rs` | New meeting, search, meeting list, Settings |
| `src/app/theme.rs` | Colours, spacing and fonts for dark and light mode |
| `src/app/platform.rs` | Finder and the folder picker |
| `src/models.rs` | Model list, folders, download and moving |
| `src/settings.rs` | Saved settings |
| `src/meeting.rs` | The Markdown meeting format (and old `.txt` transcripts) |
| `src/meetings.rs` | Meetings folder: file names, listing, search, date groups |
| `src/wav.rs` | Offline mode for WAV files |

## License

MIT
