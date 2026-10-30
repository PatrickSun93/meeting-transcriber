//! `meeting-transcriber dictate start|stop|toggle`: the running app listens
//! on a Unix socket for these, so a shortcut set in any desktop's keyboard
//! settings can dictate, also where no hotkey can be watched (Wayland
//! without the shortcuts portal). One command per connection, one line
//! each way.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use super::super::KeyEvent;
use super::super::shared::{DictateCommand, socket_path};
use super::hotkey::Events;

/// Longest either side waits for the other's line.
const WAIT: Duration = Duration::from_secs(3);

fn path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    socket_path(runtime.as_deref(), &crate::models::app_support_dir())
}

/// Starts listening, on a thread of its own, and turns each command into a
/// key event.
pub(super) fn listen(events: Arc<Events>) -> Result<()> {
    listen_at(&path(), events)
}

fn listen_at(path: &Path, events: Arc<Events>) -> Result<()> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(path).is_ok() {
                bail!("another copy of the app already listens at {}", path.display());
            }
            // Left behind by a copy that did not quit cleanly.
            std::fs::remove_file(path).with_context(|| format!("could not remove {}", path.display()))?;
            UnixListener::bind(path).with_context(|| format!("could not listen at {}", path.display()))?
        }
        Err(e) => return Err(e).with_context(|| format!("could not listen at {}", path.display())),
    };
    // Only this user may connect: XDG_RUNTIME_DIR is private already, the
    // app data folder may not be.
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    std::thread::Builder::new()
        .name("dictate-socket".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(&stream, &events);
            }
        })
        .context("could not start the dictate thread")?;
    Ok(())
}

fn serve(stream: &UnixStream, events: &Events) {
    let _ = stream.set_read_timeout(Some(WAIT));
    let _ = stream.set_write_timeout(Some(WAIT));
    let mut line = String::new();
    let _ = BufReader::new(stream.take(64)).read_line(&mut line);
    let reply = match DictateCommand::parse(&line) {
        Some(command) => {
            let down = command.press(events.is_down());
            events.send(if down { KeyEvent::Down } else { KeyEvent::Up });
            if down { "ok started\n" } else { "ok stopped\n" }.to_string()
        }
        None => format!("error unknown command {:?}; use start, stop or toggle\n", line.trim()),
    };
    let _ = (&*stream).write_all(reply.as_bytes());
}

/// `meeting-transcriber dictate <command>`: sends the command to the
/// running app and returns its answer.
pub fn send_dictate_command(command: &str) -> Result<String> {
    send_to(&path(), command)
}

fn send_to(path: &Path, command: &str) -> Result<String> {
    let Some(command) = DictateCommand::parse(command) else {
        bail!("unknown dictate command {command:?}: use start, stop or toggle");
    };
    let mut stream = UnixStream::connect(path).with_context(|| {
        format!(
            "Meeting Transcriber is not running (nothing listens at {})",
            path.display()
        )
    })?;
    stream.set_read_timeout(Some(WAIT))?;
    stream.set_write_timeout(Some(WAIT))?;
    stream.write_all(format!("{}\n", command.name()).as_bytes())?;
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .context("no answer from Meeting Transcriber")?;
    match reply.trim().strip_prefix("error ") {
        Some(why) => bail!("{why}"),
        None => Ok(reply.trim().trim_start_matches("ok ").to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn commands_reach_the_running_app() {
        let dir = std::env::temp_dir().join(format!("mt-dictate-{}", std::process::id()));
        let path = dir.join("test.sock");
        let _ = std::fs::remove_file(&path);
        assert!(send_to(&path, "toggle").is_err(), "nothing listens yet");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let events = Events::new(Box::new(move |e| record.lock().unwrap().push(e)));
        listen_at(&path, events.clone()).unwrap();
        assert_eq!(send_to(&path, "toggle").unwrap(), "started");
        assert_eq!(send_to(&path, "toggle").unwrap(), "stopped");
        assert_eq!(send_to(&path, "start").unwrap(), "started");
        assert_eq!(send_to(&path, "STOP").unwrap(), "stopped");
        assert!(send_to(&path, "dance").is_err());
        assert_eq!(
            *seen.lock().unwrap(),
            [KeyEvent::Down, KeyEvent::Up, KeyEvent::Down, KeyEvent::Up]
        );
        // A second copy does not take the socket over.
        assert!(listen_at(&path, events).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
