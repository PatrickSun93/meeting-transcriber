//! Everything that differs between macOS, Windows and Linux, behind one small
//! API: the file manager, the folder picker, and the words the window uses
//! for this computer. Each system has its own file (`macos.rs`, `windows.rs`,
//! `linux.rs`, which also serves the BSDs); the script builders are here so
//! their tests run on every system.
//!
//! Audio capture has its own per-system code in `audio::system`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crossbeam_channel::Receiver;

#[cfg(not(any(target_os = "macos", windows)))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(not(any(target_os = "macos", windows)))]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(windows)]
use windows as imp;

#[cfg(windows)]
pub use imp::attach_console;
pub use imp::{MIC_ACCESS_HINT, NO_MIC_HINT, SHOW_IN_FILES, SYSTEM_AUDIO, THIS_COMPUTER, open_folder, reveal};

/// How find-in-transcript is reached from the keyboard.
pub const FIND_SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘F" } else { "Ctrl+F" };

/// Shows the native "choose folder" dialog on a background thread, so the
/// window keeps drawing. Receives None if the user cancels.
pub fn pick_folder(prompt: &str, start: &Path, notify: impl Fn() + Send + 'static) -> Receiver<Option<PathBuf>> {
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

/// The folder a dialog printed. Cancel exits with an error or prints nothing.
#[cfg(not(target_os = "macos"))]
fn picked_path(out: std::process::Output) -> Option<PathBuf> {
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
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

/// PowerShell for the Windows Forms folder dialog. The path is written as
/// UTF-8, so folders with any name come back intact.
#[cfg(any(windows, test))]
fn windows_picker_script(prompt: &str, start: &Path) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let mut script = String::from(
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
         Add-Type -AssemblyName System.Windows.Forms; \
         $d = New-Object System.Windows.Forms.FolderBrowserDialog; ",
    );
    script += &format!("$d.Description = {}; ", quote(prompt));
    if start.is_dir() {
        script += &format!("$d.SelectedPath = {}; ", quote(&start.to_string_lossy()));
    }
    script += "if ($d.ShowDialog() -eq 'OK') { [Console]::Out.Write($d.SelectedPath) }";
    script
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
    fn windows_picker_script_quotes_text() {
        let script = windows_picker_script("Pick Patrick's folder", Path::new("/no/such/folder"));
        assert!(script.contains("$d.Description = 'Pick Patrick''s folder'; "));
        assert!(!script.contains("$d.SelectedPath = "));
        let start = std::env::temp_dir();
        assert!(windows_picker_script("x", &start).contains("$d.SelectedPath = '"));
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
