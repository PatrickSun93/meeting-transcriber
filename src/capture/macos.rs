//! Screenshots on macOS: the display under the mouse through ScreenCaptureKit
//! (macOS 14 and later; Quartz's display image on macOS 13), without this
//! app's own windows, which needs the "Screen & System Audio Recording"
//! permission; the windows on it from Quartz's window list; and the overlay
//! raised above the menu bar, with the focus handed back afterwards.

use std::cell::RefCell;
use std::ffi::c_void;

use anyhow::{Context as _, Result, anyhow, bail};
use core_foundation::base::{CFType, TCFType as _};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::display::CGDisplay;
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGRect};
use core_graphics::window::{
    kCGWindowBounds, kCGWindowLayer, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    kCGWindowOwnerPID,
};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSRunningApplication, NSScreenSaverWindowLevel,
    NSWindowCollectionBehavior, NSWorkspace,
};
use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo, NSString};
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt as _, SCScreenshotManager};

use super::image::{Image, Packing};
use super::select::PxRect;
use super::{Display, Shot};

const PERMISSION_HINT: &str = "Screenshots need the Screen Recording permission: allow Meeting Transcriber in \
     System Settings > Privacy & Security > Screen & System Audio Recording, then quit and reopen it.";

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGImageGetBitmapInfo(image: *const c_void) -> u32;
}

/// Whether this process may record the screen, for `doctor`.
pub fn allowed() -> bool {
    // SAFETY: a plain query of this process's permission.
    unsafe { CGPreflightScreenCaptureAccess() }
}

pub fn grab() -> Result<Shot> {
    if !allowed() {
        // Adds the app to the list in System Settings, switched off.
        // SAFETY: as above; it may show the system's prompt.
        unsafe { CGRequestScreenCaptureAccess() };
        bail!("{PERMISSION_HINT}");
    }
    let mouse = mouse_location()?;
    let ids = CGDisplay::active_displays().map_err(|e| anyhow!("could not list the displays (error {e})"))?;
    let id = ids
        .iter()
        .copied()
        .find(|id| contains(&CGDisplay::new(*id).bounds(), mouse))
        .unwrap_or_else(|| CGDisplay::main().id);
    let display = CGDisplay::new(id);
    let bounds = display.bounds();
    let (width, height) = display
        .display_mode()
        .map_or((display.pixels_wide(), display.pixels_high()), |m| {
            (m.pixel_width(), m.pixel_height())
        });
    let image = if at_least_macos(14) {
        screencapturekit_image(id, width as u32, height as u32)?
    } else {
        quartz_image(&display)?
    };
    let scale = image.width as f64 / bounds.size.width.max(1.0);
    Ok(Shot {
        display: Display {
            origin_px: [
                (bounds.origin.x * scale).round() as i32,
                (bounds.origin.y * scale).round() as i32,
            ],
            scale: scale as f32,
        },
        windows: windows_on(&bounds, scale, image.width, image.height),
        image,
        taken: chrono::Local::now().naive_local(),
    })
}

fn contains(rect: &CGRect, p: CGPoint) -> bool {
    p.x >= rect.origin.x
        && p.y >= rect.origin.y
        && p.x < rect.origin.x + rect.size.width
        && p.y < rect.origin.y + rect.size.height
}

/// Where the mouse is, in Quartz's global points (from the top left of the
/// main display).
fn mouse_location() -> Result<CGPoint> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| anyhow!("could not ask where the mouse is"))?;
    let event = CGEvent::new(source).map_err(|()| anyhow!("could not ask where the mouse is"))?;
    Ok(event.location())
}

fn at_least_macos(major: isize) -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: major,
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// The display through ScreenCaptureKit, at its full resolution, without
/// this app's windows or the mouse pointer.
fn screencapturekit_image(id: u32, width: u32, height: u32) -> Result<Image> {
    let content = SCShareableContent::get().map_err(|e| anyhow!("{PERMISSION_HINT} ({e})"))?;
    let display = content
        .displays()
        .into_iter()
        .find(|d| d.display_id() == id)
        .context("ScreenCaptureKit does not list the display under the mouse")?;
    let me = std::process::id() as i32;
    let ours: Vec<SCRunningApplication> = content
        .applications()
        .into_iter()
        .filter(|a| a.process_id() == me)
        .collect();
    let ours: Vec<&SCRunningApplication> = ours.iter().collect();
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_applications(&ours, &[])
        .build()
        .context("could not set up the screenshot")?;
    let config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        .with_shows_cursor(false);
    let image =
        SCScreenshotManager::capture_image(&filter, &config).map_err(|e| anyhow!("the screenshot failed: {e}"))?;
    let rgba = image
        .rgba_data()
        .map_err(|e| anyhow!("could not read the screenshot: {e}"))?;
    Ok(Image {
        width: image.width() as u32,
        height: image.height() as u32,
        rgba,
    })
}

/// The display through Quartz (macOS 13, which has no ScreenCaptureKit
/// screenshots). This app's windows are in it.
fn quartz_image(display: &CGDisplay) -> Result<Image> {
    let image = display.image().context(PERMISSION_HINT)?;
    if image.bits_per_pixel() != 32 || image.bits_per_component() != 8 {
        bail!(
            "the display's pixels are in a format this app does not read ({} bits)",
            image.bits_per_pixel()
        );
    }
    // A reference to the wrapper is the `CGImageRef` itself.
    let image_ref: *const c_void = (&*image as *const core_graphics::image::CGImageRef).cast();
    // SAFETY: the image is alive for the call.
    let info = unsafe { CGImageGetBitmapInfo(image_ref) };
    let data = image.data();
    Image::from_packed(
        image.width() as u32,
        image.height() as u32,
        image.bytes_per_row(),
        data.bytes(),
        packing(info),
    )
}

/// How a 32-bit Quartz image packs its pixels, from its bitmap info: the
/// byte order (`kCGBitmapByteOrder32Little` or big) and whether alpha (or
/// the unused byte) comes last.
fn packing(bitmap_info: u32) -> Packing {
    const BYTE_ORDER_MASK: u32 = 0x7000;
    const BYTE_ORDER_32_LITTLE: u32 = 2 << 12;
    // kCGImageAlphaPremultipliedLast, Last and NoneSkipLast.
    let alpha_last = matches!(bitmap_info & 0x1f, 1 | 3 | 5);
    let (red, green, blue) = if alpha_last {
        (0xff00_0000, 0x00ff_0000, 0x0000_ff00)
    } else {
        (0x00ff_0000, 0x0000_ff00, 0x0000_00ff)
    };
    Packing {
        red,
        green,
        blue,
        big_endian: bitmap_info & BYTE_ORDER_MASK != BYTE_ORDER_32_LITTLE,
    }
}

/// Ordinary windows of other apps on the display, front to back, in the
/// screenshot's pixels.
fn windows_on(display: &CGRect, scale: f64, width: u32, height: u32) -> Vec<PxRect> {
    let Some(list) = CGDisplay::window_list_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        None,
    ) else {
        return Vec::new();
    };
    let me = i64::from(std::process::id());
    // SAFETY: Quartz's own constant keys.
    let (bounds_key, layer_key, pid_key) = unsafe {
        (
            CFString::wrap_under_get_rule(kCGWindowBounds),
            CFString::wrap_under_get_rule(kCGWindowLayer),
            CFString::wrap_under_get_rule(kCGWindowOwnerPID),
        )
    };
    let mut windows = Vec::new();
    for item in list.iter() {
        // SAFETY: every entry of the window list is a dictionary.
        let info: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule((*item).cast()) };
        let number = |key: &CFString| {
            info.find(key)
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
        };
        if number(&layer_key) != Some(0) || number(&pid_key) == Some(me) {
            continue;
        }
        let Some(rect) = info
            .find(&bounds_key)
            .and_then(|v| v.downcast::<CFDictionary>())
            .and_then(|d| CGRect::from_dict_representation(&d))
        else {
            continue;
        };
        let px = |v: f64, origin: f64| ((v - origin) * scale).round() as i64;
        let (x0, y0) = (px(rect.origin.x, display.origin.x), px(rect.origin.y, display.origin.y));
        let (x1, y1) = (
            px(rect.origin.x + rect.size.width, display.origin.x),
            px(rect.origin.y + rect.size.height, display.origin.y),
        );
        if let Some(r) = PxRect::clipped(x0, y0, x1, y1, width, height) {
            windows.push(r);
        }
    }
    windows
}

thread_local! {
    /// The app in front before the overlay took the focus.
    static FRONT: RefCell<Option<Retained<NSRunningApplication>>> = const { RefCell::new(None) };
}

/// Remembers the app in front, to give it the focus back after the overlay.
pub fn remember_front_app() {
    let front = NSWorkspace::sharedWorkspace().frontmostApplication();
    FRONT.with(|f| *f.borrow_mut() = front);
}

/// Gives the focus back to the app that was in front, unless that was this one.
pub fn restore_front_app() {
    let Some(app) = FRONT.with(|f| f.borrow_mut().take()) else {
        return;
    };
    if *app != *NSRunningApplication::currentApplication() {
        app.activateWithOptions(NSApplicationActivationOptions::empty());
    }
}

/// Puts the overlay window (found by its title) above the menu bar and the
/// Dock, on every Space including a full-screen app's, and gives it the
/// keyboard. False while the window does not exist yet.
pub fn raise_overlay(title: &str) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    let title = NSString::from_str(title);
    let Some(window) = app.windows().iter().find(|w| w.title().isEqualToString(&title)) else {
        return false;
    };
    window.setLevel(NSScreenSaverWindowLevel);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    // `activate` needs macOS 14; the app runs on 13.
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quartz_pixel_layouts() {
        // kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little: B G R X.
        assert_eq!(packing(6 | 0x2000), Packing::BGRX);
        // kCGImageAlphaPremultipliedLast, default order: R G B A.
        let rgba = packing(1);
        assert!(rgba.big_endian);
        assert_eq!(rgba.red, 0xff00_0000);
        let image = Image::from_packed(1, 1, 4, &[10, 20, 30, 255], rgba).unwrap();
        assert_eq!(image.rgba, [10, 20, 30, 255]);
        // kCGImageAlphaNoneSkipFirst, default order: X R G B.
        let xrgb = Image::from_packed(1, 1, 4, &[0, 10, 20, 30], packing(6)).unwrap();
        assert_eq!(xrgb.rgba, [10, 20, 30, 255]);
    }
}
