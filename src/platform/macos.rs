//! macOS: Finder and an AppleScript folder dialog.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{choose_folder_script, spawn};

pub const SHOW_IN_FILES: &str = "Show in Finder";
pub const THIS_COMPUTER: &str = "this Mac";
/// What the Others source records.
pub const SYSTEM_AUDIO: &str = "Mac audio";
pub const NO_MIC_HINT: &str = "Check System Settings > Sound > Input";
pub const MIC_ACCESS_HINT: &str = "Allow microphone access in System Settings > Privacy & Security > Microphone";

/// Selects `path` in Finder.
pub fn reveal(path: &Path) {
    spawn(Command::new("open").arg("-R").arg(path));
}

/// Opens a folder in Finder, creating it first so there is something to show.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    spawn(Command::new("open").arg(dir));
}

pub(super) fn run_picker(prompt: &str, start: &Path) -> Option<PathBuf> {
    // `activate` brings the dialog in front of the app window.
    let out = Command::new("osascript")
        .args(["-e", "activate", "-e", &choose_folder_script(prompt, start)])
        .output()
        .ok()?;
    // Cancel exits with an error (-128).
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    // AppleScript ends folder paths with "/"; drop it except for the root.
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        &path
    };
    (!path.is_empty()).then(|| PathBuf::from(path))
}
