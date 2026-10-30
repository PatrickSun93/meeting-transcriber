//! The dictation hotkey on Linux, from whichever source works here:
//!
//! - X11: XInput 2 raw key events, seen whichever window has the focus. The
//!   key itself, held and let go, like on the Mac.
//! - Wayland: the desktop's global shortcuts portal. Apps may not watch the
//!   keyboard there, so the desktop asks which key to use; it reports the
//!   shortcut pressed (`Activated`) and released (`Deactivated`).
//! - Always: `meeting-transcriber dictate start|stop|toggle` (`socket.rs`),
//!   for a shortcut set in the desktop's keyboard settings.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _, KeyEventFlags, XIEventMask};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use super::super::shared::{KeySource, key_sources, portal_request_path, portal_trigger};
use super::super::{HotkeyWatch, KeyEvent};
use super::{socket, system};
use crate::dictation::hotkey::{Hotkey, x11};

/// Shown when neither a hotkey nor the `dictate` socket works; the socket
/// fails when another copy of the app holds it.
pub const HOTKEY_HINT: &str = "Quit any other copy of Meeting Transcriber, then press Try again.";

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const SHORTCUT_ID: &str = "dictate";
/// What the window calls the key while the portal has not said.
const PORTAL_WAITING: &str = "the dictation shortcut";
/// …and when there is no portal: the `dictate` command is all there is.
const NO_SHORTCUT: &str = "your dictation shortcut";

/// Set while XInput 2 events come in, so `hotkey_held` can ask X.
static X11_KEYS: AtomicBool = AtomicBool::new(false);

/// Where every source's events go, one at a time.
pub(super) struct Events {
    on_event: Mutex<Box<dyn Fn(KeyEvent) + Send>>,
    /// Whether the last event was `Down`, for `dictate toggle`.
    down: AtomicBool,
}

impl Events {
    pub(super) fn new(on_event: Box<dyn Fn(KeyEvent) + Send>) -> Arc<Self> {
        Arc::new(Self {
            on_event: Mutex::new(on_event),
            down: AtomicBool::new(false),
        })
    }

    pub(super) fn send(&self, event: KeyEvent) {
        match event {
            KeyEvent::Down => self.down.store(true, Ordering::Relaxed),
            KeyEvent::Up => self.down.store(false, Ordering::Relaxed),
            _ => {}
        }
        (self.on_event.lock().unwrap_or_else(|e| e.into_inner()))(event);
    }

    pub(super) fn is_down(&self) -> bool {
        self.down.load(Ordering::Relaxed)
    }
}

/// Whether `key` is down right now, asked of the X server rather than
/// learnt from events, so a release that was missed still shows. None
/// on Wayland, where nobody may ask.
pub fn hotkey_held(key: Hotkey) -> Option<bool> {
    static CONNECTION: Mutex<Option<RustConnection>> = Mutex::new(None);
    if !X11_KEYS.load(Ordering::Relaxed) {
        return None;
    }
    let mut connection = CONNECTION.lock().unwrap_or_else(|e| e.into_inner());
    if connection.is_none() {
        *connection = x11rb::connect(None).ok().map(|(c, _)| c);
    }
    let keymap = connection.as_ref()?.query_keymap().ok().and_then(|c| c.reply().ok());
    match keymap {
        Some(reply) => Some(x11::is_down(&reply.keys, x11::keycode(key))),
        None => {
            // Try a new connection next time.
            *connection = None;
            None
        }
    }
}

pub(in crate::platform) fn start_hotkey(watch: HotkeyWatch, on_event: Box<dyn Fn(KeyEvent) + Send>) -> Result<()> {
    let events = Events::new(on_event);
    let socket = socket::listen(events.clone());
    if let Err(e) = &socket {
        eprintln!("dictation: {e:#}");
    }
    let listening = socket.is_ok();
    let has = |var: &str| std::env::var_os(var).is_some_and(|v| !v.is_empty());
    for source in key_sources(system::wayland(), has("DISPLAY")) {
        match source {
            KeySource::X11 => match start_x11(watch.clone(), events.clone()) {
                Ok(()) => return Ok(()),
                Err(e) => eprintln!("dictation: no X11 hotkey: {e:#}"),
            },
            KeySource::Portal => {
                start_portal(watch, events, listening);
                return Ok(());
            }
        }
    }
    if let Err(e) = socket {
        bail!("Dictation can't see a hotkey here, and `meeting-transcriber dictate` can't reach it: {e:#}.");
    }
    watch.set_system_key(Some(NO_SHORTCUT.into()));
    Ok(())
}

/// Listens for raw key events on the root window: XInput 2 sends them
/// whichever window has the focus. Only reads them.
fn start_x11(watch: HotkeyWatch, events: Arc<Events>) -> Result<()> {
    let (conn, screen) = x11rb::connect(None).context("no X11 display")?;
    let version = conn
        .xinput_xi_query_version(2, 2)
        .ok()
        .and_then(|c| c.reply().ok())
        .context("the X server has no XInput 2")?;
    if version.major_version < 2 {
        bail!(
            "the X server has XInput {}.{}",
            version.major_version,
            version.minor_version
        );
    }
    let root = conn.setup().roots.get(screen).context("no X11 screen")?.root;
    conn.xinput_xi_select_events(
        root,
        &[xinput::EventMask {
            deviceid: xinput::Device::ALL_MASTER.into(),
            mask: vec![XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE],
        }],
    )?
    .check()
    .context("could not listen for key events")?;
    X11_KEYS.store(true, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || x11_loop(&conn, &watch, &events))
        .context("could not start the hotkey thread")?;
    Ok(())
}

fn x11_loop(conn: &RustConnection, watch: &HotkeyWatch, events: &Events) {
    loop {
        let (code, down, repeat) = match conn.wait_for_event() {
            Ok(Event::XinputRawKeyPress(e)) => (e.detail, true, e.flags.contains(KeyEventFlags::KEY_REPEAT)),
            Ok(Event::XinputRawKeyRelease(e)) => (e.detail, false, false),
            Ok(_) => continue,
            Err(e) => {
                eprintln!("dictation: lost the X11 display: {e}");
                break;
            }
        };
        if watch.capturing() {
            if down && !repeat {
                watch.capture_next(false);
                events.send(if code == x11::ESCAPE {
                    KeyEvent::CaptureCancelled
                } else {
                    KeyEvent::Captured(x11::from_keycode(code))
                });
            }
            continue;
        }
        if code == x11::keycode(watch.key()) && !repeat {
            events.send(if down { KeyEvent::Down } else { KeyEvent::Up });
        }
    }
    X11_KEYS.store(false, Ordering::Relaxed);
    // Whatever was held is let go of. `dictate` still works, so this is
    // not reported as the hotkey stopping.
    events.send(KeyEvent::Up);
}

/// Asks the portal for a global shortcut, on a thread of its own: binding
/// it may wait for the user to answer the desktop's dialog. Until then,
/// and if it fails, the window says what to press instead.
fn start_portal(watch: HotkeyWatch, events: Arc<Events>, listening: bool) {
    watch.set_system_key(Some(PORTAL_WAITING.into()));
    let spawned = std::thread::Builder::new().name("hotkey-portal".into()).spawn(move || {
        if let Err(e) = run_portal(&watch, &events) {
            eprintln!("dictation: no global shortcut from the desktop: {e:#}");
            if listening {
                watch.set_system_key(Some(NO_SHORTCUT.into()));
            } else {
                events.send(KeyEvent::Stopped);
            }
        }
    });
    if let Err(e) = spawned {
        eprintln!("dictation: could not start the portal thread: {e}");
    }
}

/// Creates a shortcuts session, binds the dictation shortcut and turns its
/// activations into key events, for as long as the session bus lasts.
fn run_portal(watch: &HotkeyWatch, events: &Events) -> Result<()> {
    let conn = zbus::blocking::Connection::session().context("no session bus")?;
    let portal = zbus::blocking::Proxy::new(&conn, PORTAL, PORTAL_PATH, SHORTCUTS)?;
    let created = portal_call(&conn, "CreateSession", "mt_session", |mut options| {
        options.insert("session_handle_token", Value::from("meeting_transcriber"));
        portal.call_method("CreateSession", &(options,))
    })?;
    let session = created
        .get("session_handle")
        .and_then(|v| text(v))
        .context("the portal made no session")?;
    let session = ObjectPath::try_from(session.as_str()).context("the portal's session is not an object")?;
    // Listening before binding, so no activation is missed.
    let signals = portal.receive_all_signals()?;
    let mut shortcut: HashMap<&str, Value> = HashMap::new();
    shortcut.insert("description", Value::from("Dictate: hold to talk"));
    if let Some(trigger) = portal_trigger(watch.key()) {
        shortcut.insert("preferred_trigger", Value::from(trigger));
    }
    let bound = portal_call(&conn, "BindShortcuts", "mt_bind", |options| {
        portal.call_method("BindShortcuts", &(&session, vec![(SHORTCUT_ID, shortcut)], "", options))
    })?;
    watch.set_system_key(Some(
        bound
            .get("shortcuts")
            .and_then(|list| trigger_description(list))
            .unwrap_or_else(|| PORTAL_WAITING.into()),
    ));
    for signal in signals {
        let header = signal.header();
        let Some(member) = header.member() else { continue };
        let body = signal.body();
        match member.as_str() {
            "Activated" | "Deactivated" => {
                let Ok((_, id, _, _)) = body.deserialize::<(ObjectPath, String, u64, HashMap<String, OwnedValue>)>()
                else {
                    continue;
                };
                if id == SHORTCUT_ID {
                    events.send(if member.as_str() == "Activated" {
                        KeyEvent::Down
                    } else {
                        KeyEvent::Up
                    });
                }
            }
            // The user changed it in the desktop's settings.
            "ShortcutsChanged" => {
                if let Ok((_, list)) = body.deserialize::<(ObjectPath, Vec<(String, HashMap<String, OwnedValue>)>)>()
                    && let Some((_, props)) = list.iter().find(|(id, _)| id == SHORTCUT_ID)
                    && let Some(label) = props.get("trigger_description").and_then(|v| text(v))
                    && !label.trim().is_empty()
                {
                    watch.set_system_key(Some(label));
                }
            }
            _ => {}
        }
    }
    bail!("the session bus closed")
}

/// Calls a portal method that answers later, with a `Response` signal on a
/// request object, and returns the answer. `call` gets the options to
/// send, the request's token already in them.
fn portal_call(
    conn: &zbus::blocking::Connection,
    what: &str,
    token: &str,
    call: impl FnOnce(HashMap<&str, Value>) -> zbus::Result<zbus::Message>,
) -> Result<HashMap<String, OwnedValue>> {
    let unique = conn.unique_name().context("no name on the session bus")?;
    let path = portal_request_path(unique.as_str(), token);
    let request = zbus::blocking::Proxy::new(conn, PORTAL, path.as_str(), "org.freedesktop.portal.Request")?;
    // Subscribed before the call, so a quick answer is not missed.
    let mut responses = request.receive_signal("Response")?;
    let mut options = HashMap::new();
    options.insert("handle_token", Value::from(token));
    call(options).with_context(|| format!("the desktop has no global shortcuts portal ({what} failed)"))?;
    let answer = responses.next().context("the portal did not answer")?;
    let (code, results) = answer.body().deserialize::<(u32, HashMap<String, OwnedValue>)>()?;
    match code {
        0 => Ok(results),
        1 => bail!("the shortcut was not allowed"),
        _ => bail!("the portal refused {what}"),
    }
}

/// The value inside any variant wrapping.
fn plain<'v, 'a>(value: &'v Value<'a>) -> &'v Value<'a> {
    match value {
        Value::Value(inner) => plain(inner),
        v => v,
    }
}

/// A string or object path.
fn text(value: &Value) -> Option<String> {
    match plain(value) {
        Value::Str(s) => Some(s.as_str().to_string()),
        Value::ObjectPath(p) => Some(p.as_str().to_string()),
        _ => None,
    }
}

/// How the desktop describes the dictation shortcut's keys ("Ctrl+Alt+D"),
/// from its list of shortcuts (`a(sa{sv})`).
fn trigger_description(shortcuts: &Value) -> Option<String> {
    let Value::Array(list) = plain(shortcuts) else {
        return None;
    };
    list.inner().iter().find_map(|entry| {
        let Value::Structure(entry) = plain(entry) else {
            return None;
        };
        let [Value::Str(id), Value::Dict(props)] = entry.fields() else {
            return None;
        };
        if id.as_str() != SHORTCUT_ID {
            return None;
        }
        props.iter().find_map(|(key, value)| {
            (text(key).as_deref() == Some("trigger_description"))
                .then(|| text(value))
                .flatten()
                .filter(|t| !t.trim().is_empty())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_descriptions_are_found() {
        let mut props: HashMap<String, Value> = HashMap::new();
        props.insert("description".into(), Value::from("Dictate"));
        props.insert("trigger_description".into(), Value::from("Ctrl+Alt+D"));
        // As the portal sends it: `a(sa{sv})`.
        let list = Value::from(vec![(SHORTCUT_ID.to_string(), props)]);
        assert_eq!(list.value_signature().to_string(), "a(sa{sv})");
        assert_eq!(trigger_description(&list).as_deref(), Some("Ctrl+Alt+D"));
        assert_eq!(
            trigger_description(&Value::Value(Box::new(list))).as_deref(),
            Some("Ctrl+Alt+D")
        );
        let other = Value::from(vec![("other".to_string(), HashMap::<String, Value>::new())]);
        assert_eq!(trigger_description(&other), None);
        assert_eq!(text(&Value::from("x")).as_deref(), Some("x"));
    }
}
