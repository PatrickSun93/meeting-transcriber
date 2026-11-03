# For AI agents

Notes for an AI agent (Claude Code or another) helping someone use, debug
or develop Meeting Transcriber.

## Debugging a copy someone has installed

The app is one program: `meeting-transcriber` (`meeting-transcriber.exe` on
Windows, wherever it was unzipped; inside `Meeting Transcriber.app/Contents/MacOS/`
on the Mac). Run it with a command to get answers on the command line:

```bash
meeting-transcriber doctor --json      # settings, folders, models, hotkey, permissions, problems
meeting-transcriber log --lines 300    # the end of the app's log
meeting-transcriber settings --json    # every setting
meeting-transcriber keys --once --json # ask the user to hold a key or combination, then let go
```

- Start with `doctor --json`: its `problems` list names what looks wrong
  (model not downloaded, nothing recorded, a bad hotkey setting, macOS
  permissions). On macOS permissions are reported for whatever ran the
  command (a terminal has its own), not for the app.
- `log` holds what the app writes to standard error when opened normally:
  errors from the microphone and system audio, Whisper and dictation, and
  panics. On Windows this is the only place they go. Ask the user to
  reproduce the problem, then read it.
- `keys` shows each key or combination as the hotkey watcher sees it, with
  its setting name and whether it is the dictation hotkey. It needs the
  user to press keys. It does not work on Wayland, where apps can't see
  the keyboard.
- `settings set KEY VALUE` checks the value before saving. A running app
  keeps its own copy: tell the user to quit and reopen it afterwards.

## Where things are

The app data folder holds `settings.txt`, `meeting-transcriber.log`,
`models/` (by default) and `Dictation/`:

| System | App data folder |
|---|---|
| Windows | `%LOCALAPPDATA%\Meeting Transcriber` (`C:\Users\<name>\AppData\Local\Meeting Transcriber`) |
| macOS | `~/Library/Application Support/Meeting Transcriber` |
| Linux | `~/.local/share/Meeting Transcriber` |

Transcripts go to `Documents/Meeting Transcripts` unless changed in Settings
(`meetings_dir`). `doctor` prints the real paths.

## Common problems

- **Recorded, but no transcript**: if `log` has `recording: Only silence has come from…`
  or `No audio has come from…`, the wrong device was recorded or the
  microphone is blocked (on Windows: Settings > Privacy & security >
  Microphone, including "Let desktop apps access your microphone"). Others
  records only the default output device. If audio did come in, Whisper may
  be slow on the processor: lines can arrive a minute after stopping.
- **Hotkey does nothing**: compare `doctor`'s `hotkey` with what `keys`
  prints when the user presses it.

## Developing

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` must pass (CI runs them on macOS, Windows and Linux). The
README's "Code layout" table says where each part lives.
