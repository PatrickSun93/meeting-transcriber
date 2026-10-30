//! The notification-area (tray) icon: a coloured microphone for dictation's
//! state; left-click opens the window, right-click shows a menu with the
//! latest dictations (click to copy), Pause, Open and Quit. It runs on a
//! thread of its own, with a hidden window that receives the icon's clicks.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context as _, Result, anyhow};
use crossbeam_channel::Sender;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFY_ICON_MESSAGE, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, HICON, KillTimer, MF_CHECKED, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SM_CXSMICON, SetForegroundWindow, SetTimer, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    WINDOW_EX_STYLE, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER,
    WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::super::shared::{ItemKind, icon_rgba, menu_action, menu_items, windows_menu_text};
use super::super::{TrayAction, TrayIcon, TrayMenu};

/// The icon's clicks, as `NOTIFYICONDATAW::uCallbackMessage`.
const WM_TRAY: u32 = WM_APP + 1;
/// Sent by `Tray::set`: the icon or its tooltip changed.
const WM_REFRESH: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
/// The timer that tries adding the icon again, and how often.
const ADD_TIMER: usize = 1;
const ADD_EVERY_MS: u32 = 2_000;
const ICONS: [TrayIcon; 6] = [
    TrayIcon::Loading,
    TrayIcon::Ready,
    TrayIcon::Recording,
    TrayIcon::Transcribing,
    TrayIcon::Paused,
    TrayIcon::Error,
];
/// Explorer's broadcast when the taskbar comes back after a crash: icons
/// have to be added again.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// What the window thread shows, as `Tray::set` last said.
struct Shown {
    icon: TrayIcon,
    tooltip: String,
    menu: TrayMenu,
}

struct Shared {
    shown: Mutex<Shown>,
    actions: Sender<TrayAction>,
    /// Wakes the window's frame loop to act on an action.
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn shown(&self) -> MutexGuard<'_, Shown> {
        self.shown.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn act(&self, action: TrayAction) {
        let _ = self.actions.send(action);
        (self.wake)();
    }
}

/// The window thread's side: what its window procedure works with.
struct Thread {
    shared: Arc<Shared>,
    icons: Vec<(TrayIcon, HICON)>,
}

thread_local! {
    static TRAY: RefCell<Option<Thread>> = const { RefCell::new(None) };
}

/// Runs `f` with the window thread's state. Borrowed shared, never kept:
/// the menu's own message loop calls the window procedure again.
fn with_tray<R>(f: impl FnOnce(&Thread) -> R) -> Option<R> {
    TRAY.with(|tray| tray.borrow().as_ref().map(f))
}

pub struct Tray {
    /// The hidden window, as a number: window handles are not `Send`, but
    /// posting to one is allowed from any thread.
    window: isize,
    shared: Arc<Shared>,
    last: RefCell<Option<(TrayIcon, String)>>,
}

impl Tray {
    /// Adds the icon to the notification area, on a thread of its own.
    pub fn new(actions: Sender<TrayAction>, wake: Box<dyn Fn() + Send + Sync>) -> Result<Self> {
        let shared = Arc::new(Shared {
            shown: Mutex::new(Shown {
                icon: TrayIcon::Loading,
                tooltip: "Meeting Transcriber".into(),
                menu: TrayMenu::default(),
            }),
            actions,
            wake,
        });
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        let thread_shared = shared.clone();
        std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || run(thread_shared, &ready_tx))
            .context("could not start the tray thread")?;
        let window = ready_rx
            .recv()
            .map_err(|_| anyhow!("the tray thread ended unexpectedly"))??;
        Ok(Self {
            window,
            shared,
            last: RefCell::new(None),
        })
    }

    /// Updates the icon and what the menu will show when next opened.
    pub fn set(&self, icon: TrayIcon, tooltip: &str, menu: TrayMenu) {
        let state = (icon, tooltip.to_string());
        let changed = self.last.borrow().as_ref() != Some(&state);
        {
            let mut shown = self.shared.shown();
            shown.menu = menu;
            if changed {
                shown.icon = icon;
                shown.tooltip = tooltip.to_string();
            }
        }
        if changed {
            *self.last.borrow_mut() = Some(state);
            // SAFETY: posting is allowed from any thread; a window that is
            // gone makes this fail harmlessly.
            unsafe {
                let _ = PostMessageW(Some(self.hwnd()), WM_REFRESH, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.window as *mut _)
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        // The window removes the icon and ends its thread.
        // SAFETY: as in `set`.
        unsafe {
            let _ = PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

/// The tray thread: makes the window and the icon, reports the window on
/// `ready` (or why there is none), then runs the window's message loop.
fn run(shared: Arc<Shared>, ready: &Sender<Result<isize>>) {
    let window = match create_window() {
        Ok(window) => window,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let icons = ICONS
        .iter()
        .filter_map(|icon| make_icon(*icon).ok().map(|h| (*icon, h)))
        .collect();
    TRAY.with(|tray| *tray.borrow_mut() = Some(Thread { shared, icons }));
    add_icon(window);
    let _ = ready.send(Ok(window.0 as isize));
    let mut msg = MSG::default();
    // SAFETY: a standard message loop on this thread; it ends at WM_QUIT,
    // which the window posts when it is destroyed.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            DispatchMessageW(&msg);
        }
    }
    if let Some(thread) = TRAY.with(|tray| tray.borrow_mut().take()) {
        for (_, icon) in thread.icons {
            // SAFETY: the icons were made by this thread and are no longer shown.
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
    }
}

/// A window that is never shown, only there to receive the icon's messages.
fn create_window() -> Result<HWND> {
    let class = w!("MeetingTranscriberTray");
    // SAFETY: the class describes `window_proc`, defined below; registering
    // it twice (a second tray) fails harmlessly and the first one is used.
    unsafe {
        let instance = HINSTANCE(GetModuleHandleW(None).context("no module handle")?.0);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        });
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Meeting Transcriber"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
        .context("could not create the tray's window")
    }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_TRAY => {
            match lparam.0 as u32 {
                WM_LBUTTONUP => {
                    with_tray(|t| t.shared.act(TrayAction::OpenWindow));
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(window),
                _ => {}
            }
            LRESULT(0)
        }
        WM_REFRESH => {
            notify(NIM_MODIFY, window);
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: the window is this thread's own.
            unsafe {
                let _ = DestroyWindow(window);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            notify(NIM_DELETE, window);
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == ADD_TIMER => {
            add_icon(window);
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            add_icon(window);
            LRESULT(0)
        }
        // SAFETY: the arguments are the ones this call was given.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Adds the icon. At login the taskbar may not be there yet, or too busy to
/// answer: then it is tried again every `ADD_EVERY_MS` until it works (the
/// window may already be hidden, and the icon is the way back to it), and
/// again whenever Explorer restarts (`TaskbarCreated`).
fn add_icon(window: HWND) {
    // SAFETY: the window is this thread's own; setting the timer again
    // only restarts it, and killing one that is not running fails harmlessly.
    unsafe {
        // A slow taskbar may add the icon yet report a failure: then the
        // icon is there to be updated.
        if notify(NIM_ADD, window) || notify(NIM_MODIFY, window) {
            let _ = KillTimer(Some(window), ADD_TIMER);
        } else {
            SetTimer(Some(window), ADD_TIMER, ADD_EVERY_MS, None);
        }
    }
}

/// Adds, updates or removes the icon, with the shown state and tooltip.
fn notify(message: NOTIFY_ICON_MESSAGE, window: HWND) -> bool {
    let Some((icon, tooltip)) = with_tray(|t| {
        let shown = t.shared.shown();
        let icon = t.icons.iter().find(|(i, _)| *i == shown.icon).map(|(_, h)| *h);
        (icon.unwrap_or_default(), shown.tooltip.clone())
    }) else {
        return false;
    };
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    // The tooltip holds 127 UTF-16 units and a NUL.
    let tip: Vec<u16> = tooltip.encode_utf16().take(data.szTip.len() - 1).collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    // SAFETY: `data` is filled in as the call expects and outlives it.
    unsafe { Shell_NotifyIconW(message, &data).as_bool() }
}

/// Shows the menu at the pointer and acts on the choice.
fn show_menu(window: HWND) {
    let Some(shared) = with_tray(|t| t.shared.clone()) else {
        return;
    };
    let model = shared.shown().menu.clone();
    // SAFETY: the menu is made, shown and destroyed here; its strings
    // outlive the calls that read them.
    let chosen = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for item in menu_items(&model) {
            let label = HSTRING::from(windows_menu_text(&item.label));
            let (flags, text) = match item.kind {
                ItemKind::Separator => (MF_SEPARATOR, PCWSTR::null()),
                ItemKind::Text => (MF_STRING | MF_GRAYED, PCWSTR(label.as_ptr())),
                ItemKind::Action => (MF_STRING, PCWSTR(label.as_ptr())),
                ItemKind::Check(on) => (
                    MF_STRING | if on { MF_CHECKED } else { MF_UNCHECKED },
                    PCWSTR(label.as_ptr()),
                ),
            };
            let _ = AppendMenuW(menu, flags, item.id as usize, text);
        }
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // Without this the menu does not close when clicking elsewhere.
        let _ = SetForegroundWindow(window);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            at.x,
            at.y,
            None,
            window,
            None,
        );
        // Lets the menu close properly the next time (KB135788).
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        chosen.0
    };
    if let Some(action) = u32::try_from(chosen).ok().and_then(|id| menu_action(id, &model)) {
        shared.act(action);
    }
}

/// The state's icon at the notification area's size.
fn make_icon(icon: TrayIcon) -> Result<HICON> {
    // SAFETY: a plain query.
    let size = match unsafe { GetSystemMetrics(SM_CXSMICON) } {
        s @ 16..=256 => s,
        _ => 16,
    };
    // Windows wants BGRA, rows from the top.
    let pixels: Vec<u8> = icon_rgba(icon, size as u32)
        .into_iter()
        .flat_map(|[r, g, b, a]| [b, g, r, a])
        .collect();
    // An all-clear AND mask, one bit a pixel with rows padded to 16 bits:
    // the colour's alpha decides what shows.
    let mask = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
    // SAFETY: both buffers hold `size` rows at their bit depths (32 and 1),
    // and Windows copies them into the icon.
    unsafe { CreateIcon(None, size, size, 1, 32, mask.as_ptr(), pixels.as_ptr()) }
        .context("could not make the tray icon")
}
