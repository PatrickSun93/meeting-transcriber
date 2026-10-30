//! What dictation asks of Windows besides the hotkey: pressing Ctrl+V in the
//! app in front, sounds, and the startup entry.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
};
use windows::core::{PCWSTR, w};

use super::super::Sound;
use super::super::shared::run_command;

/// Whether dictation can run here at all.
pub const DICTATION: bool = true;
/// Whether "Open at login" can be offered.
pub const AUTOSTART: bool = true;
/// Closing the window leaves the app in the notification area.
pub const HAS_TRAY: bool = true;
pub const PASTE_SHORTCUT: &str = "Ctrl+V";

/// The startup programs of this user.
const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: PCWSTR = w!("Meeting Transcriber");
/// Shift, Alt and the Windows keys: held during Ctrl+V they make it another
/// shortcut.
const STRAY_MODIFIERS: [VIRTUAL_KEY; 4] = [VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN];
/// Longest Ctrl+V waits for them to come up.
const MODIFIER_WAIT: Duration = Duration::from_millis(1500);

fn key_down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: a plain query of the keyboard's state.
    unsafe { GetAsyncKeyState(i32::from(key.0)) as u16 & 0x8000 != 0 }
}

/// Waits (a little, and at most `MODIFIER_WAIT`) until Shift, Alt and the
/// Windows keys are all up: the hotkey's own, or one held for the next thing.
fn wait_for_modifiers_up() {
    std::thread::sleep(Duration::from_millis(50));
    let deadline = Instant::now() + MODIFIER_WAIT;
    while STRAY_MODIFIERS.into_iter().any(key_down) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn key_input(key: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Presses Ctrl+V for the app in front.
pub fn paste() -> Result<()> {
    wait_for_modifiers_up();
    let inputs = [
        key_input(VK_CONTROL, false),
        key_input(VK_V, false),
        key_input(VK_V, true),
        key_input(VK_CONTROL, true),
    ];
    // SAFETY: the inputs are plain keyboard events of the given size.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        // Windows does not let a normal app type into one running as
        // administrator.
        bail!(
            "Windows did not let the app press Ctrl+V (is an administrator's window in front?). The text is on the clipboard."
        );
    }
    Ok(())
}

/// Plays one of the system's sounds by its alias, so the sound scheme in
/// Settings > Sound decides how it sounds (or that it is silent).
pub fn play_sound(sound: Sound) {
    let alias = match sound {
        Sound::Start => w!("DeviceConnect"),
        Sound::Done => w!("Notification.Default"),
        Sound::Error => w!("SystemHand"),
    };
    // SAFETY: the alias is a static string; ASYNC returns at once and
    // stops whatever this app played before.
    unsafe {
        let _ = PlaySoundW(alias, None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT);
    }
}

/// Whether the app is among this user's startup programs.
pub fn open_at_login() -> bool {
    // SAFETY: asks only whether the value is there; no buffer is given.
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS }
}

/// Adds the app to this user's startup programs (it starts hidden in the
/// notification area), or takes it off. Nothing is started now.
pub fn set_open_at_login(on: bool) -> Result<()> {
    if on {
        let exe = std::env::current_exe().context("could not find where the app is")?;
        let value: Vec<u16> = run_command(&exe).encode_utf16().chain([0]).collect();
        // SAFETY: `value` is a NUL-terminated UTF-16 string of the given
        // size in bytes.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(value.as_ptr().cast()),
                (value.len() * 2) as u32,
            )
        };
        status.ok().context("could not add the app to the startup programs")
    } else {
        // SAFETY: plain strings; removing a value that is not there fails
        // harmlessly.
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        status
            .ok()
            .context("could not remove the app from the startup programs")
    }
}

/// The hook needs no permission, so there is no settings page to open.
pub fn open_hotkey_settings() {}

/// Showing the window is all it takes.
pub fn show_app() {}

/// Hiding the window takes it off the taskbar; the tray icon stays.
pub fn hide_app() {}
