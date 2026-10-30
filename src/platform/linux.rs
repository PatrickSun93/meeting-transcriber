//! Linux and the BSDs: the desktop's file manager (over D-Bus, else
//! xdg-open) and zenity or kdialog for the folder dialog.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{file_uri, picked_path, spawn};

pub const SHOW_IN_FILES: &str = "Show in folder";
pub const THIS_COMPUTER: &str = "this computer";
/// What the Others source records.
pub const SYSTEM_AUDIO: &str = "computer audio";
pub const NO_MIC_HINT: &str = "Check the input device in your sound settings";
pub const MIC_ACCESS_HINT: &str = "Check that the input device is not muted in your sound settings";

/// Selects `path` in the file manager through the freedesktop
/// `FileManager1` interface (Files, Dolphin, Nemo, Thunar, …). Without it,
/// opens the file's folder: xdg-open has no way to select a file.
pub fn reveal(path: &Path) {
    let uri = file_uri(path);
    let dir = path.parent().map(Path::to_path_buf);
    std::thread::spawn(move || {
        let shown = Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{uri}"))
            .arg("string:")
            .output()
            .is_ok_and(|out| out.status.success());
        if !shown && let Some(dir) = dir {
            spawn(Command::new("xdg-open").arg(dir));
        }
    });
}

/// Opens a folder in the file manager, creating it first so there is
/// something to show.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    spawn(Command::new("xdg-open").arg(dir));
}

/// zenity (GNOME and most others), else kdialog (KDE).
pub(super) fn run_picker(prompt: &str, start: &Path) -> Option<PathBuf> {
    // A trailing separator makes zenity open inside the folder.
    let mut start_arg = start.as_os_str().to_owned();
    start_arg.push("/");
    let zenity = Command::new("zenity")
        .args(["--file-selection", "--directory", "--title", prompt, "--filename"])
        .arg(&start_arg)
        .output();
    let out = match zenity {
        Ok(out) => out,
        // Not installed: try KDE's dialog.
        Err(_) => Command::new("kdialog")
            .arg("--getexistingdirectory")
            .arg(start)
            .args(["--title", prompt])
            .output()
            .ok()?,
    };
    picked_path(out)
}
