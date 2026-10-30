//! macOS: Finder and an AppleScript folder dialog.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Picked, choose_folder_script, spawn};

pub const SHOW_IN_FILES: &str = "Show in Finder";
pub const THIS_COMPUTER: &str = "this Mac";
/// What the Others source records.
pub const SYSTEM_AUDIO: &str = "Mac audio";
pub const NO_MIC_HINT: &str = "Check System Settings > Sound > Input";
/// Shown above the path field that stands in for a folder dialog.
pub const NO_DIALOG_HINT: &str = "The folder dialog could not be opened, so type the folder's path.";
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

pub(super) fn run_picker(prompt: &str, start: &Path) -> Picked {
    // `activate` brings the dialog in front of the app window.
    let Ok(out) = Command::new("osascript")
        .args(["-e", "activate", "-e", &choose_folder_script(prompt, start)])
        .output()
    else {
        return Picked::NoDialog;
    };
    // Cancel exits with an error (-128).
    if !out.status.success() {
        return Picked::Cancelled;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    // AppleScript ends folder paths with "/"; drop it except for the root.
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        &path
    };
    if path.is_empty() {
        Picked::Cancelled
    } else {
        Picked::Folder(PathBuf::from(path))
    }
}
