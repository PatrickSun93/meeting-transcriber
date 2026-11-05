//! Linux and the BSDs: the desktop's file manager (over D-Bus, else
//! xdg-open) and zenity or kdialog for the folder dialog; for dictation, the
//! hotkey (`hotkey.rs`, with `socket.rs` for `meeting-transcriber
//! dictate`), the tray icon (`tray.rs`) and the rest of what it needs from
//! the system (`system.rs`).

use std::path::Path;
use std::process::Command;

use super::{Picked, file_uri, picked_path, spawn};

mod hotkey;
mod socket;
mod system;
mod tray;

pub(super) use hotkey::start_hotkey;
pub use hotkey::{HOTKEY_HINT, hotkey_held};
pub use socket::send_dictate_command;
pub use system::{
    AUTOSTART, DICTATION, HAS_TRAY, PASTE_SHORTCUT, hide_app, open_at_login, open_hotkey_settings, paste, paste_note,
    play_sound, set_open_at_login, show_app,
};
pub use tray::Tray;

const INSTALL_HOW: &str = "Run install.sh from the package (or scripts/install-linux.sh from a checkout).";

/// Installing is Windows' (the zip has no installer); here the package's
/// own way installs it.
pub fn installed() -> bool {
    true
}

pub fn running_from_temp() -> bool {
    false
}

pub fn install_dir() -> std::path::PathBuf {
    std::path::PathBuf::new()
}

pub fn install(_desktop: bool) -> anyhow::Result<std::path::PathBuf> {
    anyhow::bail!("{}", INSTALL_HOW)
}

pub fn open_installed(_exe: &std::path::Path) -> anyhow::Result<()> {
    Ok(())
}

pub fn uninstall() -> anyhow::Result<std::path::PathBuf> {
    anyhow::bail!("{}", INSTALL_HOW)
}

/// Where the hotkey can come from in this session, for
/// `meeting-transcriber doctor`.
pub fn diagnostics() -> Vec<(&'static str, String)> {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let session = if system::wayland() { "wayland" } else { "x11" };
    vec![
        ("session", session.into()),
        ("display", var("DISPLAY")),
        ("wayland_display", var("WAYLAND_DISPLAY")),
        ("desktop", var("XDG_CURRENT_DESKTOP")),
        (
            "hotkey_source",
            if system::wayland() {
                "the desktop's global shortcuts portal, or `dictate` bound to a shortcut".into()
            } else {
                "XInput 2 raw key events".into()
            },
        ),
    ]
}

pub const SHOW_IN_FILES: &str = "Show in folder";
pub const THIS_COMPUTER: &str = "this computer";
/// What the Others source records.
pub const SYSTEM_AUDIO: &str = "computer audio";
pub const NO_MIC_HINT: &str = "Check the input device in your sound settings";
/// Shown above the path field that stands in for a folder dialog.
pub const NO_DIALOG_HINT: &str = "No folder dialog is installed (zenity or kdialog), so type the folder's path.";
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

/// Opens a file in the desktop's app for it (a recording in the audio player).
pub fn open_file(path: &Path) {
    spawn(Command::new("xdg-open").arg(path));
}

/// Opens a folder in the file manager, creating it first so there is
/// something to show.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    spawn(Command::new("xdg-open").arg(dir));
}

/// KDE's dialog, for a folder or (with patterns) a file.
fn kdialog(prompt: &str, start: &Path, patterns: Option<&str>) -> Command {
    let mut kdialog = Command::new("kdialog");
    match patterns {
        Some(p) => kdialog
            .arg("--getopenfilename")
            .arg(start)
            .arg(format!("{p}|Recordings")),
        None => kdialog.arg("--getexistingdirectory").arg(start),
    };
    kdialog.args(["--title", prompt]);
    kdialog
}

/// zenity (GNOME and most others), else kdialog (KDE). With neither, the
/// window asks for the path as text.
pub(super) fn run_picker(prompt: &str, start: &Path, files: Option<&[&str]>) -> Picked {
    // A trailing separator makes zenity open inside the folder.
    let mut start_arg = start.as_os_str().to_owned();
    start_arg.push("/");
    // Both dialogs match patterns case-sensitively.
    let patterns = files.map(|exts| {
        exts.iter()
            .flat_map(|e| [format!("*.{e}"), format!("*.{}", e.to_uppercase())])
            .collect::<Vec<_>>()
            .join(" ")
    });
    let mut zenity = Command::new("zenity");
    zenity.args(["--file-selection", "--title", prompt]);
    match &patterns {
        Some(p) => zenity.arg(format!("--file-filter=Recordings | {p}")),
        None => zenity.arg("--directory"),
    };
    let out = match zenity.arg("--filename").arg(&start_arg).output() {
        Ok(out) => out,
        // Not installed: try KDE's dialog.
        Err(_) => match kdialog(prompt, start, patterns.as_deref()).output() {
            Ok(out) => out,
            Err(_) => return Picked::NoDialog,
        },
    };
    picked_path(out)
}
