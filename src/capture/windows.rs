//! Screenshots on Windows: the monitor under the mouse copied with GDI
//! (`BitBlt` from the screen, in physical pixels: winit makes the app
//! per-monitor DPI aware), its scale from its DPI, and the windows on it
//! from `EnumWindows` with their visible frames from DWM. No permission is
//! needed.

use std::cell::Cell;
use std::ffi::c_void;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetWindowLongW, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, SetForegroundWindow, WS_EX_TRANSPARENT,
};
use windows::core::{BOOL, HSTRING, PCWSTR};

use super::image::{Image, Packing};
use super::select::PxRect;
use super::{Display, Shot};

pub fn grab() -> Result<Shot> {
    let mut cursor = POINT::default();
    // SAFETY: plain queries with structures of the documented sizes.
    let (monitor, scale) = unsafe {
        GetCursorPos(&mut cursor).context("could not ask where the mouse is")?;
        let handle = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(handle, &mut info).as_bool() {
            bail!("could not find the monitor under the mouse");
        }
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let scale = match GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) {
            Ok(()) => dpi_x as f32 / 96.0,
            Err(_) => 1.0,
        };
        (info.rcMonitor, scale)
    };
    let image = copy_screen(monitor)?;
    let windows = windows_on(monitor, image.width, image.height);
    Ok(Shot {
        display: Display {
            origin_px: [monitor.left, monitor.top],
            scale,
        },
        windows,
        image,
        taken: chrono::Local::now().naive_local(),
    })
}

/// The pixels of `rect` of the virtual screen.
fn copy_screen(rect: RECT) -> Result<Image> {
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    if width <= 0 || height <= 0 {
        bail!("the monitor under the mouse has no size");
    }
    let mut data = vec![0u8; width as usize * height as usize * 4];
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative: rows from the top.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: every handle made here is checked and released here; the
    // bitmap is selected out of the memory DC before GetDIBits reads it,
    // and `data` holds `height` rows of `width` 32-bit pixels.
    let (copied, lines) = unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            bail!("could not read the screen");
        }
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let mut result = (false, 0);
        if !memory.is_invalid() && !bitmap.is_invalid() {
            let old = SelectObject(memory, bitmap.into());
            let copied = BitBlt(
                memory,
                0,
                0,
                width,
                height,
                Some(screen),
                rect.left,
                rect.top,
                SRCCOPY | CAPTUREBLT,
            )
            .is_ok();
            let _ = SelectObject(memory, old);
            let lines = if copied {
                GetDIBits(
                    memory,
                    bitmap,
                    0,
                    height as u32,
                    Some(data.as_mut_ptr().cast::<c_void>()),
                    &mut info,
                    DIB_RGB_COLORS,
                )
            } else {
                0
            };
            result = (copied, lines);
        }
        if !bitmap.is_invalid() {
            let _ = DeleteObject(bitmap.into());
        }
        if !memory.is_invalid() {
            let _ = DeleteDC(memory);
        }
        ReleaseDC(None, screen);
        result
    };
    if !copied || lines != height {
        bail!("could not copy the screen");
    }
    Image::from_packed(width as u32, height as u32, width as usize * 4, &data, Packing::BGRX)
}

/// What `EnumWindows` hands each window: where to put what it finds.
struct Search {
    process: u32,
    monitor: RECT,
    width: u32,
    height: u32,
    found: Vec<PxRect>,
}

/// Other apps' visible windows on the monitor, front to back (the order
/// `EnumWindows` goes in), in the screenshot's pixels.
fn windows_on(monitor: RECT, width: u32, height: u32) -> Vec<PxRect> {
    let mut search = Search {
        process: std::process::id(),
        monitor,
        width,
        height,
        found: Vec::new(),
    };
    // SAFETY: the callback only runs during the call, and `search` outlives it.
    unsafe {
        let _ = EnumWindows(Some(each_window), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

unsafe extern "system" fn each_window(window: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `Search` handed to EnumWindows, alive for the call.
    let search = unsafe { &mut *(lparam.0 as *mut Search) };
    // SAFETY: plain queries of a window EnumWindows just named, with
    // buffers of the sizes given.
    unsafe {
        let mut process = 0u32;
        GetWindowThreadProcessId(window, Some(&mut process));
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        );
        let see_through = GetWindowLongW(window, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0;
        let shown = IsWindowVisible(window).as_bool() && !IsIconic(window).as_bool() && cloaked == 0;
        if !shown || see_through || process == search.process || GetWindowTextLengthW(window) == 0 {
            return BOOL(1);
        }
        let mut frame = RECT::default();
        if DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut frame as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
        {
            let m = search.monitor;
            if let Some(r) = PxRect::clipped(
                i64::from(frame.left - m.left),
                i64::from(frame.top - m.top),
                i64::from(frame.right - m.left),
                i64::from(frame.bottom - m.top),
                search.width,
                search.height,
            ) {
                search.found.push(r);
            }
        }
    }
    BOOL(1)
}

thread_local! {
    /// The window in front before the overlay took the focus.
    static FRONT: Cell<Option<HWND>> = const { Cell::new(None) };
}

/// Remembers the window in front, to give it the focus back after the overlay.
pub fn remember_front_app() {
    // SAFETY: a plain query.
    let front = unsafe { GetForegroundWindow() };
    FRONT.with(|f| f.set((!front.is_invalid()).then_some(front)));
}

/// Gives the focus back to the window that was in front, if it is still there.
pub fn restore_front_app() {
    if let Some(window) = FRONT.with(Cell::take) {
        // SAFETY: `IsWindow` checks the handle before it is used.
        unsafe {
            if IsWindow(Some(window)).as_bool() {
                let _ = SetForegroundWindow(window);
            }
        }
    }
}

/// Brings the overlay window (found by its title) to the front with the
/// keyboard; it is always on top already. False while it does not exist yet.
pub fn raise_overlay(title: &str) -> bool {
    // SAFETY: a lookup by title, and a request about the window found.
    unsafe {
        match FindWindowW(PCWSTR::null(), &HSTRING::from(title)) {
            Ok(window) if !window.is_invalid() => {
                let _ = SetForegroundWindow(window);
                true
            }
            _ => false,
        }
    }
}
