//! Dictation's system parts on Windows and Linux: not there yet. Each says
//! so, and the rest of the app works without them. The functions match the
//! macOS ones (`macos/`), so a backend can replace this file piece by piece.

use anyhow::{Result, bail};
use crossbeam_channel::Sender;

use super::{HotkeyWatch, KeyEvent, Sound, TrayAction, TrayIcon, TrayMenu};

const NOT_YET: &str = if cfg!(windows) {
    "is not supported on Windows yet"
} else {
    "is not supported on Linux yet"
};

/// Whether dictation can run here at all.
pub const DICTATION: bool = false;
/// Whether "Open at login" can be offered.
pub const AUTOSTART: bool = false;
/// Whether closing the window can leave the app running in the tray.
pub const HAS_TRAY: bool = false;
pub const PASTE_SHORTCUT: &str = "Ctrl+V";
/// Shown when the hotkey cannot be watched.
pub const HOTKEY_HINT: &str = "";

pub(super) fn start_hotkey(_watch: HotkeyWatch, _on_event: Box<dyn Fn(KeyEvent) + Send>) -> Result<()> {
    bail!("The dictation hotkey {NOT_YET}.")
}

/// Pastes into the app in front.
pub fn paste() -> Result<()> {
    bail!("Pasting {NOT_YET}; the text is on the clipboard.")
}

pub fn play_sound(_sound: Sound) {}

pub fn open_at_login() -> bool {
    false
}

pub fn set_open_at_login(_on: bool) -> Result<()> {
    bail!("Opening at login {NOT_YET}.")
}

/// Opens the system settings page for the hotkey permission.
pub fn open_hotkey_settings() {}

/// Shows the app (after the window was hidden to the tray).
pub fn show_app() {}

/// Called when the window is hidden to the tray.
pub fn hide_app() {}

/// The tray icon.
pub struct Tray;

impl Tray {
    pub fn new(_actions: Sender<TrayAction>, _wake: Box<dyn Fn()>) -> Result<Self> {
        bail!("The tray icon {NOT_YET}.")
    }

    pub fn set(&self, _icon: TrayIcon, _tooltip: &str, _menu: TrayMenu) {}
}
