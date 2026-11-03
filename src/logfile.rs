//! The app's log: what it writes to standard error (errors from the audio,
//! Whisper and dictation threads, panics), kept in `meeting-transcriber.log`
//! in the app data folder when nobody would see it otherwise. On Windows
//! the app has no console at all, so without this every message is lost.
//! `meeting-transcriber log` prints the end of it, `doctor` names it.

use std::fs::{File, OpenOptions};
use std::io::{IsTerminal as _, Write as _};
use std::path::PathBuf;

/// The log starts afresh past this size; the previous one is kept as `.old`.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub fn log_path() -> PathBuf {
    crate::models::app_support_dir().join("meeting-transcriber.log")
}

/// Sends this process's standard error to the log file, unless it goes to a
/// terminal (started from one, where it is read as it comes). Failing leaves
/// standard error as it was.
pub fn start() {
    if std::io::stderr().is_terminal() {
        return;
    }
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = writeln!(
        file,
        "\n=== {} Meeting Transcriber {} on {} {} ===",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    redirect_stderr(file);
}

#[cfg(unix)]
fn redirect_stderr(file: File) {
    use std::os::fd::IntoRawFd as _;
    unsafe extern "C" {
        fn dup2(old: i32, new: i32) -> i32;
    }
    let fd = file.into_raw_fd();
    // SAFETY: `fd` is an open file this process owns, kept open for good;
    // 2 is standard error.
    unsafe {
        dup2(fd, 2);
    }
}

#[cfg(windows)]
fn redirect_stderr(file: File) {
    use std::os::windows::io::IntoRawHandle as _;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetStdHandle(which: u32, handle: *mut std::ffi::c_void) -> i32;
    }
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    let handle = file.into_raw_handle();
    // SAFETY: `handle` is an open file this process owns, kept open for
    // good. Rust's standard error asks for the handle at every write.
    unsafe {
        SetStdHandle(STD_ERROR_HANDLE, handle);
    }
}

#[cfg(not(any(unix, windows)))]
fn redirect_stderr(_file: File) {}

/// The last `lines` lines of the log (and of the one before, if the log
/// is shorter), or None when there is none.
pub fn tail(lines: usize) -> Option<String> {
    let path = log_path();
    let mut text = std::fs::read_to_string(path.with_extension("log.old")).unwrap_or_default();
    text.push_str(&std::fs::read_to_string(&path).ok()?);
    let all: Vec<&str> = text.lines().collect();
    Some(all[all.len().saturating_sub(lines)..].join("\n"))
}
