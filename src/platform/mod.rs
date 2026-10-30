//! Everything that differs between macOS, Windows and Linux, behind one small
//! API: the file manager, the folder picker, the words the window uses for
//! this computer, and what dictation needs from the system (the global
//! hotkey, pasting, sounds, the menu bar item, opening at login). Each system
//! has its own file (`macos/`, `windows.rs`, `linux.rs`, which also serves
//! the BSDs); the script builders are here so their tests run on every
//! system.
//!
//! Dictation's system parts exist for macOS so far. On Windows and Linux
//! they are in `unsupported.rs` and say so, until their backends are added.
//!
//! Audio capture has its own per-system code in `audio::system`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use crossbeam_channel::Receiver;

use crate::dictation::hotkey::Hotkey;

#[cfg(not(any(target_os = "macos", windows)))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(windows)]
mod windows;

#[cfg(not(any(target_os = "macos", windows)))]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(windows)]
use windows as imp;

/// Dictation's system parts.
#[cfg(target_os = "macos")]
use macos as dictation_imp;
#[cfg(not(target_os = "macos"))]
use unsupported as dictation_imp;

#[cfg(windows)]
pub use imp::attach_console;
pub use imp::{
    MIC_ACCESS_HINT, NO_DIALOG_HINT, NO_MIC_HINT, SHOW_IN_FILES, SYSTEM_AUDIO, THIS_COMPUTER, open_file, open_folder,
    reveal,
};

pub use dictation_imp::{
    AUTOSTART, DICTATION, HAS_TRAY, HOTKEY_HINT, PASTE_SHORTCUT, Tray, hide_app, open_at_login, open_hotkey_settings,
    paste, play_sound, set_open_at_login, show_app,
};

/// What the hotkey watcher reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// The hotkey went down…
    Down,
    /// …and came back up.
    Up,
    /// The key pressed while capturing (see `HotkeyWatch::capture_next`).
    Captured(Hotkey),
    /// Escape was pressed while capturing.
    CaptureCancelled,
}

/// The running hotkey watcher's settings, shared with its thread.
#[derive(Clone, Default)]
pub struct HotkeyWatch {
    key: Arc<Mutex<Hotkey>>,
    capture: Arc<AtomicBool>,
}

impl HotkeyWatch {
    pub fn key(&self) -> Hotkey {
        *self.key.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_key(&self, key: Hotkey) {
        *self.key.lock().unwrap_or_else(|e| e.into_inner()) = key;
    }

    /// The next key pressed is reported as `Captured` instead of being
    /// compared with the hotkey.
    pub fn capture_next(&self, on: bool) {
        self.capture.store(on, Ordering::Relaxed);
    }

    pub fn capturing(&self) -> bool {
        self.capture.load(Ordering::Relaxed)
    }
}

/// Starts watching for `key` system-wide, on a thread of its own, and calls
/// `on_event` there. Fails when the system does not allow it (on macOS,
/// without Input Monitoring), with what to do about it.
pub fn watch_hotkey(key: Hotkey, on_event: impl Fn(KeyEvent) + Send + 'static) -> Result<HotkeyWatch> {
    let watch = HotkeyWatch::default();
    watch.set_key(key);
    dictation_imp::start_hotkey(watch.clone(), Box::new(on_event))?;
    Ok(watch)
}

/// Dictation's sounds: voice2cursor's Tink, Pop and Basso on the Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Start,
    Done,
    Error,
}

/// The menu bar item's state, shown by its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIcon {
    Loading,
    Ready,
    Recording,
    Transcribing,
    Paused,
    Error,
}

/// What the menu bar item's menu shows; it is rebuilt from this when opened.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrayMenu {
    pub status: String,
    /// The latest dictations, newest first: time and text.
    pub recent: Vec<(String, String)>,
    pub paused: bool,
}

/// What was chosen in the menu bar item's menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    Copy(String),
    TogglePause,
    OpenWindow,
    Quit,
}

/// Puts `text` on the clipboard. The clipboard handle is kept: on Linux
/// (X11) the text is only there while its owner lives.
pub fn copy_text(text: &str) -> Result<()> {
    static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);
    let mut clipboard = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().context("could not open the clipboard")?);
    }
    let result = clipboard
        .as_mut()
        .map(|c| c.set_text(text.to_string()))
        .context("no clipboard")?;
    if result.is_err() {
        // Try a fresh handle next time.
        *clipboard = None;
    }
    result.context("could not copy to the clipboard")
}

/// How find-in-transcript is reached from the keyboard.
pub const FIND_SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘F" } else { "Ctrl+F" };

/// What a folder dialog came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Folder(PathBuf),
    Cancelled,
    /// There is no dialog to show (on Linux, neither zenity nor kdialog is
    /// installed), so the caller asks for the path as text instead.
    NoDialog,
}

/// Shows the native "choose folder" dialog on a background thread, so the
/// window keeps drawing.
pub fn pick_folder(prompt: &str, start: &Path, notify: impl Fn() + Send + 'static) -> Receiver<Picked> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let (prompt, start) = (prompt.to_string(), start.to_path_buf());
    std::thread::spawn(move || {
        let _ = tx.send(imp::run_picker(&prompt, &start));
        notify();
    });
    rx
}

/// Starts a helper without waiting for it, and reaps it on a thread so it
/// does not linger as a zombie process.
fn spawn(command: &mut Command) {
    if let Ok(mut child) = command.spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// The folder zenity or kdialog printed. Cancel exits with an error or
/// prints nothing.
#[cfg(any(not(any(target_os = "macos", windows)), all(test, unix)))]
fn picked_path(out: std::process::Output) -> Picked {
    if !out.status.success() {
        return Picked::Cancelled;
    }
    let path = String::from_utf8_lossy(&out.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if path.is_empty() {
        Picked::Cancelled
    } else {
        Picked::Folder(PathBuf::from(path))
    }
}

/// AppleScript for the macOS dialog, starting in `start` when it exists.
#[cfg(any(target_os = "macos", test))]
fn choose_folder_script(prompt: &str, start: &Path) -> String {
    let mut script = format!(
        "POSIX path of (choose folder with prompt {}",
        applescript_string(prompt)
    );
    if start.is_dir() {
        script += &format!(
            " default location (POSIX file {})",
            applescript_string(&start.to_string_lossy())
        );
    }
    script.push(')');
    script
}

#[cfg(any(target_os = "macos", test))]
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A launchd agent that opens the app at login, in the background (the
/// window stays hidden; the menu bar item shows it).
#[cfg(any(target_os = "macos", test))]
fn launch_agent_plist(label: &str, app: &Path) -> String {
    let xml = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/open</string>
    <string>-a</string>
    <string>{}</string>
    <string>--args</string>
    <string>--hidden</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#,
        xml(label),
        xml(&app.to_string_lossy())
    )
}

/// The path with `\` for every `/`. Windows accepts both, but Explorer
/// does not: given `D:/Models` or `C:\Users\me\Models/old` (a folder
/// typed in Settings), it opens its default folder instead. Names cannot contain `/`.
#[cfg(any(windows, test))]
fn backslashed(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace('/', "\\"))
}

/// A `file://` URI for a local path, as the freedesktop file manager
/// interface wants it: every byte outside the unreserved set and `/` is
/// percent-encoded, so spaces and non-ASCII names survive.
#[cfg(any(not(any(target_os = "macos", windows)), all(test, unix)))]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => uri.push(b as char),
            _ => uri += &format!("%{b:02X}"),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_script_quotes_text() {
        let script = choose_folder_script(r#"Pick "the" folder \ now"#, Path::new("/no/such/folder"));
        assert_eq!(
            script,
            r#"POSIX path of (choose folder with prompt "Pick \"the\" folder \\ now")"#
        );
        let start = std::env::temp_dir();
        assert!(choose_folder_script("x", &start).contains(" default location (POSIX file \""));
    }

    #[test]
    fn login_agent_opens_the_app_hidden() {
        let plist = launch_agent_plist("com.example.app", Path::new("/Applications/A & B.app"));
        assert!(plist.contains("<key>Label</key><string>com.example.app</string>"));
        assert!(plist.contains("<string>/Applications/A &amp; B.app</string>"));
        assert!(plist.contains("<string>--hidden</string>"));
        assert!(plist.contains("<key>RunAtLoad</key><true/>"));
        assert!(!plist.contains("/Volumes"));
    }

    #[test]
    fn hotkey_watch_shares_its_settings() {
        let watch = HotkeyWatch::default();
        let thread_side = watch.clone();
        watch.set_key(Hotkey::F(13));
        watch.capture_next(true);
        assert_eq!(thread_side.key(), Hotkey::F(13));
        assert!(thread_side.capturing());
    }

    #[test]
    fn explorer_gets_backslashes() {
        assert_eq!(
            backslashed(Path::new(r"C:\Users\me\Models/old")),
            PathBuf::from(r"C:\Users\me\Models\old")
        );
        assert_eq!(backslashed(Path::new("D:/Models")), PathBuf::from(r"D:\Models"));
    }

    #[cfg(unix)]
    #[test]
    fn dialog_output_becomes_a_folder_or_a_cancel() {
        use std::os::unix::process::ExitStatusExt as _;
        let output = |code: i32, stdout: &str| std::process::Output {
            // A wait status: the exit code sits in the second byte.
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.into(),
            stderr: Vec::new(),
        };
        assert_eq!(
            picked_path(output(0, "/home/me/My Meetings\n")),
            Picked::Folder(PathBuf::from("/home/me/My Meetings"))
        );
        assert_eq!(
            picked_path(output(0, "C:\\Users\\me\r\n")),
            Picked::Folder(PathBuf::from("C:\\Users\\me"))
        );
        // zenity and kdialog exit with 1 on Cancel.
        assert_eq!(picked_path(output(1, "")), Picked::Cancelled);
        assert_eq!(picked_path(output(0, "")), Picked::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn file_uris_are_percent_encoded() {
        assert_eq!(
            file_uri(Path::new("/home/me/Meeting Transcripts/周会 #1.md")),
            "file:///home/me/Meeting%20Transcripts/%E5%91%A8%E4%BC%9A%20%231.md"
        );
        assert_eq!(file_uri(Path::new("/a-b_c.d~e")), "file:///a-b_c.d~e");
    }
}
