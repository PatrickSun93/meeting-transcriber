//! Windows: Explorer, a Windows Forms folder dialog, and the console.

use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::process::Command;

use super::{Picked, backslashed, picked_path, spawn, windows_picker_script};

pub const SHOW_IN_FILES: &str = "Show in Explorer";
pub const THIS_COMPUTER: &str = "this PC";
/// What the Others source records.
pub const SYSTEM_AUDIO: &str = "PC audio";
pub const NO_MIC_HINT: &str = "Check Settings > System > Sound > Input";
/// Shown above the path field that stands in for a folder dialog.
pub const NO_DIALOG_HINT: &str =
    "The folder dialog could not be opened (PowerShell may be blocked), so type the folder's path.";
pub const MIC_ACCESS_HINT: &str = "Allow microphone access in Settings > Privacy & security > Microphone, \
     including \"Let desktop apps access your microphone\"";

/// No console window flashing up behind a helper.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Selects `path` in Explorer.
pub fn reveal(path: &Path) {
    // Explorer wants `/select,"<path>"` as written; Rust's own quoting
    // would wrap the whole argument and Explorer would not parse it.
    // (Windows paths cannot contain quotes.)
    let mut arg = std::ffi::OsString::from("/select,\"");
    arg.push(backslashed(path));
    arg.push("\"");
    spawn(Command::new("explorer").raw_arg(arg));
}

/// Opens a folder in Explorer, creating it first so there is something to show.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    spawn(Command::new("explorer").arg(backslashed(dir)));
}

pub(super) fn run_picker(prompt: &str, start: &Path) -> Picked {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-STA",
            "-Command",
            &windows_picker_script(prompt, &backslashed(start)),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    // PowerShell ships with Windows, but a locked-down PC may block it.
    out.map_or(Picked::NoDialog, picked_path)
}

/// Connects stdout and stderr to the console the app was started from, if
/// any. Release builds are GUI programs with no console window of their own,
/// so without this `meeting-transcriber transcribe` would print nothing.
pub fn attach_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: takes no pointers, and fails harmlessly (returns 0) when the
    // app was not started from a console.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
