//! The system file manager and the native folder picker: Finder and an
//! AppleScript dialog on macOS, Explorer and a Windows Forms dialog on
//! Windows, the desktop's file manager and zenity or kdialog on Linux.

use std::path::{Path, PathBuf};
use std::process::Command;

use crossbeam_channel::Receiver;

/// The button that shows a file in the file manager.
#[cfg(target_os = "macos")]
pub const SHOW_IN_FILES: &str = "Show in Finder";
#[cfg(windows)]
pub const SHOW_IN_FILES: &str = "Show in Explorer";
#[cfg(not(any(target_os = "macos", windows)))]
pub const SHOW_IN_FILES: &str = "Show in folder";

/// How find-in-transcript is reached from the keyboard.
pub const FIND_SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘F" } else { "Ctrl+F" };

/// Selects `path` in the file manager (Linux opens its folder: there is no
/// common way to select a file).
pub fn reveal_in_finder(path: &Path) {
    #[cfg(target_os = "macos")]
    spawn(Command::new("open").arg("-R").arg(path));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // Explorer wants `/select,"<path>"` as written; Rust's own quoting
        // would wrap the whole argument and Explorer would not parse it.
        // (Windows paths cannot contain quotes.)
        let mut arg = std::ffi::OsString::from("/select,\"");
        arg.push(path);
        arg.push("\"");
        spawn(Command::new("explorer").raw_arg(arg));
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    if let Some(dir) = path.parent() {
        spawn(Command::new("xdg-open").arg(dir));
    }
}

/// Opens a folder in the file manager, creating it first so there is
/// something to show.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(target_os = "macos")]
    spawn(Command::new("open").arg(dir));
    #[cfg(windows)]
    spawn(Command::new("explorer").arg(dir));
    #[cfg(not(any(target_os = "macos", windows)))]
    spawn(Command::new("xdg-open").arg(dir));
}

/// Starts a helper without waiting for it, and reaps it on a thread so it
/// does not linger as a zombie process.
fn spawn(command: &mut Command) {
    if let Ok(mut child) = command.spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// Shows the native "choose folder" dialog on a background thread, so the
/// window keeps drawing. Receives None if the user cancels.
pub fn pick_folder(prompt: &str, start: &Path, notify: impl Fn() + Send + 'static) -> Receiver<Option<PathBuf>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let (prompt, start) = (prompt.to_string(), start.to_path_buf());
    std::thread::spawn(move || {
        let _ = tx.send(run_picker(&prompt, &start));
        notify();
    });
    rx
}

/// AppleScript for the dialog, starting in `start` when it exists.
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

#[cfg(target_os = "macos")]
fn run_picker(prompt: &str, start: &Path) -> Option<PathBuf> {
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

#[cfg(windows)]
fn run_picker(prompt: &str, start: &Path) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt as _;
    // No console window flashing up behind the dialog.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = Command::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", &windows_picker_script(prompt, start)])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    picked_path(out)
}

/// zenity (GNOME and most others), else kdialog (KDE).
#[cfg(not(any(target_os = "macos", windows)))]
fn run_picker(prompt: &str, start: &Path) -> Option<PathBuf> {
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
}
