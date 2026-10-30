//! The global hotkey on Windows: a low-level keyboard hook (`WH_KEYBOARD_LL`)
//! on a thread of its own, which runs the message loop the hook is called
//! from. It only reads key events and passes every one on; it needs no
//! permission.

use std::cell::RefCell;

use anyhow::{Context as _, Result, anyhow};
use crossbeam_channel::Sender;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_UP, MSG,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
};

use super::super::{HotkeyWatch, KeyEvent};
use crate::dictation::hotkey::{Hotkey, win};

/// The hook needs no permission, so there is nothing to add.
pub const HOTKEY_HINT: &str = "";

/// What the hook procedure works with. It has no argument for it, so it
/// lives with the hook's thread.
struct Hook {
    watch: HotkeyWatch,
    on_event: Box<dyn Fn(KeyEvent) + Send>,
    /// The hotkey's code while it is down, to tell repeats from presses.
    held: Option<u32>,
}

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

/// Whether `key` is down right now, asked of the keyboard rather than
/// learnt from events, so a release the hook missed still shows.
pub fn hotkey_held(key: Hotkey) -> Option<bool> {
    let code = i32::try_from(win::vk(key)).ok()?;
    // SAFETY: a plain query; any key code is accepted. The high bit is set
    // while the key is down.
    Some(unsafe { GetAsyncKeyState(code) } as u16 & 0x8000 != 0)
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

/// Installs the hook and runs this thread's message loop, which calls it.
/// Reports on `ready` whether the hook could be installed, and with
/// `Stopped` if the loop ever ends.
fn run_hook(watch: HotkeyWatch, on_event: Box<dyn Fn(KeyEvent) + Send>, ready: &Sender<Result<()>>) {
    HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Hook {
            watch,
            on_event,
            held: None,
        })
    });
    // SAFETY: `hook_proc` has the signature Windows calls, and lives as
    // long as the program. The module handle is this program's own.
    let installed = unsafe {
        let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0)
    };
    let hook = match installed {
        Ok(hook) => hook,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("Windows did not allow watching the keyboard: {e}")));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut msg = MSG::default();
    // SAFETY: a standard message loop on this thread; GetMessageW returns
    // 0 at WM_QUIT and -1 on an error, either of which ends it.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
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
        // Keys typed by programs (this one's Ctrl+V, a remote desktop's
        // replay) are not the user's.
        if info.flags.0 & LLKHF_INJECTED.0 == 0 {
            let up = info.flags.0 & LLKHF_UP.0 != 0;
            HOOK.with(|hook| {
                if let Some(hook) = hook.borrow_mut().as_mut() {
                    on_key(hook, info.vkCode, up);
                }
            });
        }
    }
    // Every key is passed on: the hook only listens.
    // SAFETY: the arguments are the ones this call was given.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn on_key(hook: &mut Hook, code: u32, up: bool) {
    if hook.watch.capturing() {
        if !up {
            hook.watch.capture_next(false);
            (hook.on_event)(if code == win::ESCAPE {
                KeyEvent::CaptureCancelled
            } else {
                KeyEvent::Captured(win::from_vk(code))
            });
        }
        return;
    }
    // Passed on as they come: dictation ignores a second Down or a stray
    // Up, and a press after a missed release still counts.
    match win::transition(win::vk(hook.watch.key()), code, up, &mut hook.held) {
        Some(true) => (hook.on_event)(KeyEvent::Down),
        Some(false) => (hook.on_event)(KeyEvent::Up),
        None => {}
    }
}
