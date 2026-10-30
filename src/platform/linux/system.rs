//! What dictation asks of Linux besides the hotkey: pressing Ctrl+V in the
//! app in front (XTest on X11; wtype or ydotool on Wayland), sounds from
//! the freedesktop theme, and the autostart entry.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _, KeyButMask};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::super::shared::{PasteTool, autostart_entry, keycode_for, paste_tools, sound_name};
use super::super::{Sound, spawn};

/// Whether dictation can run here at all.
pub const DICTATION: bool = true;
/// Whether "Open at login" can be offered.
pub const AUTOSTART: bool = true;
/// The tray icon needs a desktop that shows StatusNotifierItems (GNOME only
/// does with the AppIndicator extension). Without one, closing the window
/// quits, as it always did, and that is not reported as an error.
pub const HAS_TRAY: bool = false;
pub const PASTE_SHORTCUT: &str = "Ctrl+V";

const XK_V: u32 = 0x76;
const XK_CONTROL_L: u32 = 0xffe3;
/// Longest Ctrl+V waits for the hotkey's modifiers to come up.
const MODIFIER_WAIT: Duration = Duration::from_millis(1500);
/// Where the freedesktop sound theme keeps its sounds.
const SOUNDS: &str = "/usr/share/sounds/freedesktop/stereo";

/// Whether this is a Wayland session.
pub(super) fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
}

fn gnome() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_lowercase().contains("gnome"))
}

/// Whether `program` is on the PATH.
fn installed(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            std::fs::metadata(dir.join(program)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

/// Under "Paste automatically" on Wayland: what presses Ctrl+V, or that
/// nothing can.
pub fn paste_note() -> Option<String> {
    if !wayland() {
        return None;
    }
    Some(
        match paste_tools(true, gnome(), installed).first() {
            Some(PasteTool::Ydotool) => "On Wayland, ydotool presses Ctrl+V; its service (ydotoold) must be running.",
            Some(_) => "On Wayland, wtype presses Ctrl+V.",
            None => "On Wayland, pasting needs wtype or ydotool; without them the text is only copied.",
        }
        .into(),
    )
}

/// Presses Ctrl+V for the app in front, with the first tool that works.
pub fn paste() -> Result<()> {
    let tools = paste_tools(wayland(), gnome(), installed);
    if tools.is_empty() {
        // Said once, not after every dictation; the Dictation settings say
        // it too.
        static TOLD: AtomicBool = AtomicBool::new(false);
        if !TOLD.swap(true, Ordering::Relaxed) {
            bail!("Pasting on Wayland needs wtype or ydotool. The text is on the clipboard.");
        }
        return Ok(());
    }
    let mut failed = Vec::new();
    for tool in tools {
        let result = match tool {
            PasteTool::XTest => xtest_paste(),
            PasteTool::Wtype => run_after_modifiers("wtype", &["-M", "ctrl", "v", "-m", "ctrl"]),
            // Linux input codes: 29 left Ctrl, 47 V; `:1` down, `:0` up.
            PasteTool::Ydotool => run_after_modifiers("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"]),
        };
        match result {
            Ok(()) => return Ok(()),
            Err(e) => failed.push(format!("{e:#}")),
        }
    }
    bail!("Could not paste ({}). The text is on the clipboard.", failed.join("; "))
}

/// Runs a typing tool once the hotkey has had a moment to come up (on
/// Wayland nobody may ask whether it has).
fn run_after_modifiers(program: &str, args: &[&str]) -> Result<()> {
    std::thread::sleep(Duration::from_millis(150));
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("could not run {program}"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        bail!("{program} failed: {}", why.trim());
    }
    Ok(())
}

/// Ctrl+V through the XTest extension, with the key codes the keyboard
/// layout has for Control_L and `v`.
fn xtest_paste() -> Result<()> {
    let (conn, screen) = x11rb::connect(None).context("no X11 display")?;
    let root = conn.setup().roots.get(screen).context("no X11 screen")?.root;
    conn.xtest_get_version(2, 2)?
        .reply()
        .map_err(|_| anyhow!("the X server has no XTEST extension"))?;
    let (first, last) = (conn.setup().min_keycode, conn.setup().max_keycode);
    let map = conn.get_keyboard_mapping(first, last - first + 1)?.reply()?;
    let keycode =
        |keysym, default| keycode_for(keysym, first, map.keysyms_per_keycode, &map.keysyms).unwrap_or(default);
    let (ctrl, v) = (keycode(XK_CONTROL_L, 37), keycode(XK_V, 55));
    wait_for_modifiers_up(&conn, root);
    for (kind, code) in [
        (xproto::KEY_PRESS_EVENT, ctrl),
        (xproto::KEY_PRESS_EVENT, v),
        (xproto::KEY_RELEASE_EVENT, v),
        (xproto::KEY_RELEASE_EVENT, ctrl),
    ] {
        conn.xtest_fake_input(kind, code, x11rb::CURRENT_TIME, root, 0, 0, 0)?;
    }
    // A round trip, so the keys are sent before the connection closes.
    conn.get_input_focus()?.reply()?;
    Ok(())
}

/// Waits (a little, and at most `MODIFIER_WAIT`) until Shift, Alt and Super
/// are all up: held during Ctrl+V they make it another shortcut.
fn wait_for_modifiers_up(conn: &RustConnection, root: xproto::Window) {
    let stray = u16::from(KeyButMask::SHIFT | KeyButMask::MOD1 | KeyButMask::MOD4);
    std::thread::sleep(Duration::from_millis(50));
    let deadline = Instant::now() + MODIFIER_WAIT;
    while Instant::now() < deadline {
        let held = conn
            .query_pointer(root)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| u16::from(r.mask) & stray != 0);
        if !held {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// How sounds are played here, looked up once.
enum Player {
    /// libcanberra's player, by sound name: follows the desktop's theme.
    Canberra,
    /// A sound file from the freedesktop theme, with this program.
    File(&'static str),
    None,
}

/// Plays a sound from the freedesktop theme, where something can play it.
pub fn play_sound(sound: Sound) {
    static PLAYER: OnceLock<Player> = OnceLock::new();
    let player = PLAYER.get_or_init(|| {
        if installed("canberra-gtk-play") {
            Player::Canberra
        } else if let Some(program) = ["paplay", "pw-play"].into_iter().find(|p| installed(p)) {
            Player::File(program)
        } else {
            Player::None
        }
    });
    let name = sound_name(sound);
    match player {
        Player::Canberra => spawn(Command::new("canberra-gtk-play").args(["-i", name])),
        Player::File(program) => {
            let file = Path::new(SOUNDS).join(format!("{name}.oga"));
            if file.is_file() {
                spawn(Command::new(program).arg(file));
            }
        }
        Player::None => {}
    }
}

fn autostart_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("autostart")
            .join("meeting-transcriber.desktop"),
    )
}

/// Whether the autostart entry is in place.
pub fn open_at_login() -> bool {
    autostart_path().is_some_and(|p| p.is_file())
}

/// Adds or removes `~/.config/autostart/meeting-transcriber.desktop`,
/// which starts the app (from where it is now) when you log in. Nothing is
/// started now.
pub fn set_open_at_login(on: bool) -> Result<()> {
    let path = autostart_path().context("no config folder")?;
    if on {
        let exe = std::env::current_exe().context("could not find where the app is")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
        }
        std::fs::write(&path, autostart_entry(&exe)).with_context(|| format!("could not write {}", path.display()))
    } else {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("could not remove {}", path.display()))
            }
            _ => Ok(()),
        }
    }
}

/// No permission is asked for, so there is no settings page to open.
pub fn open_hotkey_settings() {}

/// Showing the window is all it takes.
pub fn show_app() {}

/// Hiding the window is all it takes; the tray icon stays.
pub fn hide_app() {}
