//! Noticing the old voice2cursor tool. It listens for the same hotkey, so
//! with both running every dictation would be pasted twice.

use std::path::{Path, PathBuf};

/// Where to find the script that stops it, for the banner.
pub const STOP_SCRIPT: &str = "/Volumes/externalssd/devitems/tools/voice2cursor/stop.sh";

/// Shown while dictation stays paused because of it.
pub fn banner() -> String {
    format!("voice2cursor is running and uses the same hotkey. Stop it with {STOP_SCRIPT}, then resume dictation here.")
}

/// The lock file voice2cursor holds while it runs.
fn lock_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".voice2cursor").join("lock"))
}

/// Whether voice2cursor is running: its single-instance lock is held, or a
/// process runs it.
pub fn is_running() -> bool {
    lock_path().is_some_and(|p| lock_is_held(&p)) || process_running()
}

/// True if another process holds an exclusive lock on `path`. Takes and
/// drops the lock itself when it is free; never creates the file.
pub fn lock_is_held(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(_)) => false,
    }
}

/// Whether a command line is voice2cursor's: `python -m voice2cursor` (the
/// cross-platform version) or `python voice2cursor.py`.
pub fn is_voice2cursor_command(command: &str) -> bool {
    let mut words = command.split_whitespace().peekable();
    let Some(program) = words.next() else { return false };
    if !program
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|p| p.starts_with("python"))
    {
        return false;
    }
    while let Some(word) = words.next() {
        if word == "-m" && words.peek() == Some(&"voice2cursor") {
            return true;
        }
        if word.rsplit(['/', '\\']).next() == Some("voice2cursor.py") {
            return !command.contains("--keytest");
        }
    }
    false
}

#[cfg(unix)]
fn process_running() -> bool {
    std::process::Command::new("ps")
        .args(["-axo", "command="])
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(is_voice2cursor_command)
        })
}

/// Not looked for yet on Windows; the lock above is not used there either.
#[cfg(not(unix))]
fn process_running() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_voice2cursor_command_lines() {
        assert!(is_voice2cursor_command(
            "/Volumes/externalssd/devitems/tools/voice2cursor/.venv/bin/python3 -m voice2cursor"
        ));
        assert!(is_voice2cursor_command("python3 voice2cursor.py"));
        assert!(is_voice2cursor_command(
            "/usr/bin/python3 /x/voice2cursor/voice2cursor.py"
        ));
        assert!(!is_voice2cursor_command("python3 voice2cursor.py --keytest"));
        assert!(!is_voice2cursor_command(
            "/x/.venv/bin/python3 -c from multiprocessing.resource_tracker import main;main(11)"
        ));
        assert!(!is_voice2cursor_command("grep -m voice2cursor"));
        assert!(!is_voice2cursor_command("vim voice2cursor.py"));
        assert!(!is_voice2cursor_command(""));
    }

    #[test]
    fn lock_held_only_by_someone_else() {
        let path = std::env::temp_dir().join(format!("meeting-transcriber-test-{}-lock", std::process::id()));
        assert!(!lock_is_held(&path), "no file");
        std::fs::write(&path, "123").unwrap();
        assert!(!lock_is_held(&path), "free");
        let holder = std::fs::File::open(&path).unwrap();
        holder.lock().unwrap();
        assert!(lock_is_held(&path), "held");
        drop(holder);
        assert!(!lock_is_held(&path), "released");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn banner_names_the_stop_script() {
        assert!(banner().contains(STOP_SCRIPT));
    }
}
