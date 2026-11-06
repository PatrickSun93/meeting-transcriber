//! The global hotkey on Windows: a low-level keyboard hook (`WH_KEYBOARD_LL`)
//! on a thread of its own, which runs the message loop the hook is called
//! from. It needs no permission. It passes every key event on, except the
//! plain keys of a combination while it is held (the D of Ctrl+Shift+D,
//! the 2 of the screenshot's Ctrl+Shift+2), so holding it to talk does not
//! type into the app in front.
//!
//! Windows silently removes a low-level hook that is ever too slow to
//! answer, and nothing tells the app. So the thread runs at the highest
//! priority (Whisper keeps every core busy while it transcribes) and puts
//! the hook in afresh now and then, which revives one that was removed.

use std::cell::RefCell;

use anyhow::{Context as _, Result, anyhow};
use crossbeam_channel::Sender;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_UP, MSG,
    PostThreadMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_TIMER,
};

use super::super::{HotkeyWatch, KeyEvent, PressTracker};
use crate::dictation::hotkey::{Mods, Outcome, Shortcut, Tracker, win};

/// The hook needs no permission, so there is nothing to add.
pub const HOTKEY_HINT: &str = "";
/// How often the hook is put in afresh, in milliseconds.
const REHOOK_EVERY_MS: u32 = 30_000;
/// Posted to the hook's thread to type the mask key (see `mask`).
const WM_MASK: u32 = WM_APP + 1;
/// An unassigned virtual-key code: typed, it does nothing.
const MASK_KEY: u16 = 0xE8;

/// What the hook procedure works with. It has no argument for it, so it
/// lives with the hook's thread.
struct Hook {
    watch: HotkeyWatch,
    on_event: Box<dyn Fn(KeyEvent) + Send>,
    tracker: Tracker,
    presses: PressTracker,
    /// Keys whose press was kept from the app in front: their repeats and
    /// release are kept from it too.
    swallowed: Vec<u32>,
    /// Keys whose press the app in front saw: so are their repeats and
    /// release, or it would think them stuck down.
    passed: Vec<u32>,
}

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

/// Whether the plain keys of `key` are kept from the app in front while it
/// is held: when it is a combination, not a single key (F13 still reaches
/// apps, as it always did).
fn swallows(key: &Shortcut) -> bool {
    key.single().is_none()
}

/// Whether `key` is held right now, asked of the keyboard rather than
/// learnt from events, so a release the hook missed still shows. Keys the
/// hook keeps from apps never show as down there, so only the rest are
/// asked about.
pub fn hotkey_held(key: &Shortcut) -> Option<bool> {
    let hidden = |code: u32| swallows(key) && win::modifier(code).is_empty();
    Some(key.held_now(&win::CODES, |code| {
        hidden(code)
            || i32::try_from(code).is_ok_and(|code| {
                // SAFETY: a plain query; any key code is accepted. The high
                // bit is set while the key is down.
                unsafe { GetAsyncKeyState(code) as u16 & 0x8000 != 0 }
            })
    }))
}

pub(in crate::platform) fn start_hotkey(watch: HotkeyWatch, on_event: Box<dyn Fn(KeyEvent) + Send>) -> Result<()> {
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || run_hook(watch, on_event, &ready_tx))
        .context("could not start the hotkey thread")?;
    ready_rx
        .recv()
        .map_err(|_| anyhow!("the hotkey thread ended unexpectedly"))?
}

/// Installs the hook for this thread.
fn install() -> windows::core::Result<HHOOK> {
    // SAFETY: `hook_proc` has the signature Windows calls, and lives as
    // long as the program. The module handle is this program's own.
    unsafe {
        let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0)
    }
}

/// Installs the hook and runs this thread's message loop, which calls it.
/// Reports on `ready` whether the hook could be installed, and with
/// `Stopped` if the loop ever ends.
fn run_hook(watch: HotkeyWatch, on_event: Box<dyn Fn(KeyEvent) + Send>, ready: &Sender<Result<()>>) {
    HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Hook {
            watch,
            on_event,
            tracker: Tracker::default(),
            presses: PressTracker::default(),
            swallowed: Vec::new(),
            passed: Vec::new(),
        })
    });
    // SAFETY: the pseudo-handle of this thread. Failing leaves it as it is.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
    let mut hook = match install() {
        Ok(hook) => hook,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("Windows did not allow watching the keyboard: {e}")));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut msg = MSG::default();
    // SAFETY: a standard message loop on this thread; GetMessageW returns
    // 0 at WM_QUIT and -1 on an error, either of which ends it. The timer
    // has no window and no callback, so it arrives here as WM_TIMER.
    unsafe {
        SetTimer(None, 0, REHOOK_EVERY_MS, None);
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if msg.message == WM_TIMER && msg.hwnd.is_invalid() {
                // The new hook goes in before the old one comes out; no key
                // is handled in between, as only this loop calls the hook.
                if let Ok(fresh) = install() {
                    let _ = UnhookWindowsHookEx(hook);
                    hook = fresh;
                }
                continue;
            }
            if msg.message == WM_MASK && msg.hwnd.is_invalid() {
                mask();
                continue;
            }
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
    }
    HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            // Whatever was held is let go of, rather than recording on.
            (hook.on_event)(KeyEvent::Up);
            (hook.on_event)(KeyEvent::Stopped);
        }
    });
}

/// Called by Windows for every key event, on the hook's thread.
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to the event's description.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        // Keys typed by programs (this one's Ctrl+V and mask key, a remote
        // desktop's replay) are not the user's.
        if info.flags.0 & LLKHF_INJECTED.0 == 0 {
            let up = info.flags.0 & LLKHF_UP.0 != 0;
            let swallow = HOOK.with(|hook| {
                hook.borrow_mut()
                    .as_mut()
                    .is_some_and(|hook| on_key(hook, info.vkCode, up))
            });
            if swallow {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: the arguments are the ones this call was given.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Takes in a key event; true when the app in front must not see it.
fn on_key(hook: &mut Hook, code: u32, up: bool) -> bool {
    let target = hook.watch.key();
    let capturing = hook.watch.capturing();
    for press in hook.presses.key(&win::CODES, &hook.watch, code, !up) {
        (hook.on_event)(KeyEvent::Pressed(press));
    }
    match hook.tracker.key(&win::CODES, &target, capturing, code, !up) {
        Some(Outcome::Down) => (hook.on_event)(KeyEvent::Down),
        Some(Outcome::Up) => (hook.on_event)(KeyEvent::Up),
        Some(Outcome::Captured(shortcut)) => {
            hook.watch.capture_next(false);
            (hook.on_event)(KeyEvent::Captured(shortcut));
        }
        Some(Outcome::CaptureCancelled) => {
            hook.watch.capture_next(false);
            (hook.on_event)(KeyEvent::CaptureCancelled);
        }
        None => {}
    }
    if up {
        hook.passed.retain(|c| *c != code);
        let swallowed = hook.swallowed.contains(&code);
        hook.swallowed.retain(|c| *c != code);
        return swallowed;
    }
    if hook.swallowed.contains(&code) {
        return true;
    }
    if hook.passed.contains(&code) {
        return false;
    }
    // A fresh press: kept from the app when it is a plain key of the
    // combination now held, dictation's or a press shortcut's.
    let dictation_key = swallows(&target)
        && hook.tracker.active()
        && win::modifier(code).is_empty()
        && target.keys.iter().any(|k| win::vk(*k) == code);
    let swallow = !capturing && (dictation_key || hook.presses.holds_plain_key(&win::CODES, &hook.watch, code));
    if swallow {
        hook.swallowed.push(code);
        // Alt or the Windows key pressed and let go of with nothing seen
        // in between opens the menu bar or Start: something must be.
        let held = hook.tracker.held_mods(&win::CODES, &[]);
        if held.contains(Mods::ALT) || held.contains(Mods::SUPER) {
            // Typed from the message loop, not from inside the hook.
            // SAFETY: a message to this thread's own queue.
            unsafe {
                let _ = PostThreadMessageW(GetCurrentThreadId(), WM_MASK, WPARAM(0), LPARAM(0));
            }
        }
    } else {
        hook.passed.push(code);
    }
    swallow
}

/// Types the mask key, which does nothing, so that letting go of Alt or the
/// Windows key does not count as pressing it alone.
fn mask() {
    let key = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(MASK_KEY),
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: plain keyboard events of the given size.
    unsafe {
        SendInput(&[key(false), key(true)], std::mem::size_of::<INPUT>() as i32);
    }
}
