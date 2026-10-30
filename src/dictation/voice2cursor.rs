//! Noticing the old voice2cursor tool. It listens for the same hotkey, so
//! with both running every dictation would be pasted twice.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

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

/// One look for voice2cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Check {
    pub running: bool,
    /// The look was asked for (see `watch`), rather than a periodic one.
    pub asked: bool,
}

/// Looks for voice2cursor now and then on a thread of its own (it runs
/// `ps`, too slow for the window's thread), so one started later, by
/// launchd say, is noticed too. Sending on the returned sender asks for a
/// look at once. Only asked looks and changes are sent, each followed by
/// `notify`. The thread ends when the sender or the receiver is dropped.
pub fn watch(every: Duration, notify: impl Fn() + Send + 'static) -> (Sender<()>, Receiver<Check>) {
    watch_with(every, is_running, notify)
}

fn watch_with(
    every: Duration,
    look: impl Fn() -> bool + Send + 'static,
    notify: impl Fn() + Send + 'static,
) -> (Sender<()>, Receiver<Check>) {
    let (ask_tx, ask_rx) = crossbeam_channel::bounded(1);
    let (tx, rx) = crossbeam_channel::unbounded();
    let spawned = std::thread::Builder::new()
        .name("voice2cursor-watch".into())
        .spawn(move || {
            let (mut asked, mut last) = (false, None);
            loop {
                let running = look();
                if asked || last != Some(running) {
                    if tx.send(Check { running, asked }).is_err() {
                        break;
                    }
                    notify();
                }
                last = Some(running);
                asked = match ask_rx.recv_timeout(every) {
                    Ok(()) => true,
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => break,
                };
            }
        });
    if let Err(e) = spawned {
        eprintln!("could not watch for voice2cursor: {e}");
    }
    (ask_tx, rx)
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
    fn watcher_sends_changes_and_asked_looks() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let running = Arc::new(AtomicBool::new(false));
        let r = running.clone();
        let (ask, checks) = watch_with(Duration::from_millis(5), move || r.load(Ordering::SeqCst), || {});
        let next = || checks.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            next(),
            Check {
                running: false,
                asked: false
            },
            "the first look"
        );
        running.store(true, Ordering::SeqCst);
        assert_eq!(
            next(),
            Check {
                running: true,
                asked: false
            },
            "a change"
        );
        // Unchanged periodic looks are not sent; an asked one is.
        std::thread::sleep(Duration::from_millis(30));
        ask.send(()).unwrap();
        assert_eq!(
            next(),
            Check {
                running: true,
                asked: true
            }
        );
        drop(ask);
        assert!(
            checks.recv_timeout(Duration::from_secs(5)).is_err(),
            "ends with the sender"
        );
    }

    #[test]
    fn banner_names_the_stop_script() {
        assert!(banner().contains(STOP_SCRIPT));
    }
}
