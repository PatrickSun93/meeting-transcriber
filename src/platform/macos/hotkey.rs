//! The global hotkey on macOS: a listen-only Quartz event tap on a thread of
//! its own, with its own run loop. It only reads key events (it never
//! changes or swallows them), which needs Input Monitoring permission.

use std::cell::Cell;
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow, bail};
use core_foundation::base::TCFType as _;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType, EventField,
};
use crossbeam_channel::Sender;

use super::super::{HotkeyWatch, KeyEvent};
use crate::dictation::hotkey::Hotkey;
use crate::dictation::hotkey::mac::{self, KeyEventKind};

pub const HOTKEY_HINT: &str = "Allow Meeting Transcriber in System Settings > Privacy & Security > Input Monitoring, \
     then press Try again (or reopen the app).";

const ESCAPE: u32 = 53;
/// `kCGEventSourceStateHIDSystemState`: the keyboard as it is now.
const HID_SYSTEM_STATE: i32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
}

/// Whether `key` is down right now, asked of the keyboard rather than
/// learnt from events, so a release the tap missed still shows.
pub fn hotkey_held(key: Hotkey) -> Option<bool> {
    let code = u16::try_from(mac::keycode(key)).ok()?;
    // SAFETY: a plain query; any key code is accepted.
    Some(unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, code) })
}

pub(in crate::platform) fn start_hotkey(watch: HotkeyWatch, on_event: Box<dyn Fn(KeyEvent) + Send>) -> Result<()> {
    // SAFETY: plain queries of this process's permission, no arguments.
    if !unsafe { CGPreflightListenEventAccess() } {
        // Adds the app to the Input Monitoring list (switched off), so
        // there is something to switch on.
        unsafe { CGRequestListenEventAccess() };
        bail!("Dictation can't see the hotkey without Input Monitoring permission.");
    }
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || run_tap(&watch, &*on_event, &ready_tx))
        .context("could not start the hotkey thread")?;
    ready_rx
        .recv()
        .map_err(|_| anyhow!("the hotkey thread ended unexpectedly"))?
}

/// Creates the tap and runs this thread's run loop until the tap can't be
/// kept on. Reports on `ready` whether the tap could be made, and with
/// `Stopped` when it ends, so that is never silent.
fn run_tap(watch: &HotkeyWatch, on_event: &dyn Fn(KeyEvent), ready: &Sender<Result<()>>) {
    // The tap's own port, for turning it back on: macOS turns off a tap
    // whose callback is slow, and tells it so with one last event.
    let port: Rc<Cell<CFMachPortRef>> = Rc::new(Cell::new(std::ptr::null_mut()));
    let tap_port = port.clone();
    let tap = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged],
        move |_proxy, kind, event| {
            let kind = match kind {
                CGEventType::KeyDown => KeyEventKind::Down,
                CGEventType::KeyUp => KeyEventKind::Up,
                CGEventType::FlagsChanged => KeyEventKind::FlagsChanged,
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                    // SAFETY: plain query of this process's permission.
                    if unsafe { CGPreflightListenEventAccess() } {
                        // SAFETY: the port is the tap's own, alive while it runs.
                        unsafe { CGEventTapEnable(tap_port.get(), true) };
                    } else {
                        // Input Monitoring was switched off: nothing more
                        // will come, so end and say so.
                        CFRunLoop::get_current().stop();
                    }
                    return None;
                }
                _ => return None,
            };
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u32;
            let flags = event.get_flags().bits();
            let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
            if watch.capturing() {
                if mac::transition(code, kind, code, flags, repeat) == Some(true) {
                    watch.capture_next(false);
                    on_event(if code == ESCAPE {
                        KeyEvent::CaptureCancelled
                    } else {
                        KeyEvent::Captured(mac::from_keycode(code))
                    });
                }
                return None;
            }
            // Passed on as they come: dictation ignores a second Down or
            // a stray Up, and a press after a missed release still counts.
            match mac::transition(mac::keycode(watch.key()), kind, code, flags, repeat) {
                Some(true) => on_event(KeyEvent::Down),
                Some(false) => on_event(KeyEvent::Up),
                None => {}
            }
            None
        },
    );
    let Ok(tap) = tap else {
        let _ = ready.send(Err(anyhow!(
            "Dictation can't see the hotkey: macOS did not allow watching the keyboard."
        )));
        return;
    };
    port.set(tap.mach_port.as_concrete_TypeRef());
    let Ok(source) = tap.mach_port.create_runloop_source(0) else {
        let _ = ready.send(Err(anyhow!("could not attach the hotkey watcher")));
        return;
    };
    // SAFETY: a constant CFString owned by Core Foundation.
    CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
    tap.enable();
    let _ = ready.send(Ok(()));
    CFRunLoop::run_current();
    // Whatever was held is let go of, rather than recording on.
    on_event(KeyEvent::Up);
    on_event(KeyEvent::Stopped);
}
