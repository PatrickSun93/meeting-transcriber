//! What dictation asks of macOS besides the hotkey: pressing ⌘V in the app
//! in front, sounds, the Dock icon, and the login item.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

use super::super::{Sound, launch_agent_plist, spawn};

/// Whether dictation can run here at all.
pub const DICTATION: bool = true;
/// Whether "Open at login" can be offered.
pub const AUTOSTART: bool = true;
/// Closing the window leaves the app in the menu bar.
pub const HAS_TRAY: bool = true;
pub const PASTE_SHORTCUT: &str = "⌘V";

const AGENT_LABEL: &str = "com.patricksun.meeting-transcriber";
/// Where `scripts/bundle-macos.sh --install` puts the app.
const APP_PATH: &str = "/Applications/Meeting Transcriber.app";
/// `kVK_ANSI_V`.
const KEY_V: u16 = 9;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

/// Presses ⌘V for the app in front. Needs Accessibility permission.
pub fn paste() -> Result<()> {
    // SAFETY: plain queries of this process's permission, no arguments.
    if !unsafe { CGPreflightPostEventAccess() } {
        unsafe { CGRequestPostEventAccess() };
        bail!(
            "Auto paste needs Accessibility permission: allow Meeting Transcriber in System Settings > \
             Privacy & Security > Accessibility. The text is on the clipboard."
        );
    }
    // Lets the hotkey's modifier come all the way up first, so this is not
    // read as ⌘⌥V.
    std::thread::sleep(Duration::from_millis(100));
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| anyhow!("could not make a keyboard event"))?;
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(source.clone(), KEY_V, down)
            .map_err(|()| anyhow!("could not make a keyboard event"))?;
        event.set_flags(CGEventFlags::CGEventFlagCommand);
        event.post(CGEventTapLocation::HID);
    }
    Ok(())
}

pub fn play_sound(sound: Sound) {
    let name = match sound {
        Sound::Start => "Tink",
        Sound::Done => "Pop",
        Sound::Error => "Basso",
    };
    spawn(Command::new("afplay").arg(format!("/System/Library/Sounds/{name}.aiff")));
}

fn agent_path() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join("Library/LaunchAgents")
            .join(format!("{AGENT_LABEL}.plist")),
    )
}

/// Whether the login item is in place.
pub fn open_at_login() -> bool {
    agent_path().is_some_and(|p| p.is_file())
}

/// Adds or removes the login item: a launchd agent that opens the app from
/// Applications when you log in. Nothing is started now.
pub fn set_open_at_login(on: bool) -> Result<()> {
    let path = agent_path().context("no home folder")?;
    if on {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
        }
        std::fs::write(&path, launch_agent_plist(AGENT_LABEL, Path::new(APP_PATH)))
            .with_context(|| format!("could not write {}", path.display()))
    } else {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("could not remove {}", path.display()))
            }
            _ => Ok(()),
        }
    }
}

/// Opens Privacy & Security > Input Monitoring.
pub fn open_hotkey_settings() {
    spawn(Command::new("open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"));
}

/// Brings the app back to the Dock and to the front.
pub fn show_app() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    // `activate` needs macOS 14; the app runs on 13.
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

/// With the window hidden, the app lives in the menu bar only.
pub fn hide_app() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    NSApplication::sharedApplication(mtm).setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}
