//! Finder and the native folder picker.

use std::path::{Path, PathBuf};

use crossbeam_channel::Receiver;

/// Selects `path` in a Finder window.
pub fn reveal_in_finder(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(target_os = "macos"))]
    if let Some(dir) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Opens a folder in Finder, creating it first so there is something to show.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(not(target_os = "macos"))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

/// Shows the native "choose folder" dialog on a background thread, so the
/// window keeps drawing. Receives None if the user cancels.
pub fn pick_folder(prompt: &str, start: &Path, notify: impl Fn() + Send + 'static) -> Receiver<Option<PathBuf>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let script = choose_folder_script(prompt, start);
    std::thread::spawn(move || {
        let _ = tx.send(run_picker(&script));
        notify();
    });
    rx
}

/// AppleScript for the dialog, starting in `start` when it exists.
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

fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
fn run_picker(script: &str) -> Option<PathBuf> {
    // `activate` brings the dialog in front of the app window.
    let out = std::process::Command::new("osascript")
        .args(["-e", "activate", "-e", script])
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

#[cfg(not(target_os = "macos"))]
fn run_picker(_script: &str) -> Option<PathBuf> {
    None
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
}
