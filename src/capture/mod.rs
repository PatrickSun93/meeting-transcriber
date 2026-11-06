//! Screenshots: grabbing the display under the mouse with the windows on
//! it (each system in its own file: `macos.rs`, `windows.rs`, `linux.rs`),
//! and the plain logic around it, here so its tests run everywhere: the
//! pictures and PNG files (`image.rs`), the selection's geometry
//! (`select.rs`), and where screenshots are saved and how notes link them
//! (`files.rs`). A scrolling screenshot takes an area again and again as it
//! scrolls (`scrolling.rs`) and stitches the pictures (`stitch.rs`). The
//! overlay that shows a screenshot is the window's (`app/overlay.rs`).

pub mod files;
pub mod image;
pub mod scrolling;
pub mod select;
pub mod stitch;

#[cfg(not(any(target_os = "macos", windows)))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(not(any(target_os = "macos", windows)))]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(windows)]
use windows as imp;

pub use imp::{hide_from_captures, raise_overlay, remember_front_app, restore_front_app};

use crate::dictation::hotkey::{Hotkey, Mods, Shortcut};
use image::Image;
use select::PxRect;

/// Where a captured display is on the desktop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    /// Its top-left corner in the desktop's pixels: on the Mac, its corner
    /// in points times `scale`.
    pub origin_px: [i32; 2],
    /// Pixels per point, as the system scales windows there.
    pub scale: f32,
}

/// One display, frozen as it was.
#[derive(Debug, Clone)]
pub struct Shot {
    /// Every pixel of the display.
    pub image: Image,
    pub display: Display,
    /// Other apps' windows on it, front to back, in `image`'s pixels (none
    /// where the system does not say).
    pub windows: Vec<PxRect>,
    pub taken: chrono::NaiveDateTime,
}

/// Grabs the display under the mouse. Slow-ish (tens of milliseconds) and
/// blocking: called on a thread of its own. Fails without permission (the
/// Mac's Screen Recording) or where screenshots are not supported, saying so.
pub fn grab() -> anyhow::Result<Shot> {
    imp::grab()
}

/// Why screenshots cannot be taken here, if they cannot (Wayland).
pub fn unsupported() -> Option<&'static str> {
    #[cfg(not(any(target_os = "macos", windows)))]
    return linux::unsupported();
    #[cfg(any(target_os = "macos", windows))]
    None
}

/// Whether the Mac lets this process record the screen, for `doctor`.
#[cfg(target_os = "macos")]
pub fn screen_recording_allowed() -> bool {
    macos::allowed()
}

/// The screenshot hotkey until one is chosen: ⌘⇧2 on the Mac (next to the
/// system's ⌘⇧3, ⌘⇧4 and ⌘⇧5), Ctrl+Shift+2 elsewhere.
pub fn default_hotkey() -> Shortcut {
    // The 2 key's code on each system's keyboards.
    let (mods, two) = if cfg!(target_os = "macos") {
        (Mods::SUPER.union(Mods::SHIFT), 19)
    } else if cfg!(windows) {
        (Mods::CTRL.union(Mods::SHIFT), 0x32)
    } else {
        (Mods::CTRL.union(Mods::SHIFT), 11)
    };
    Shortcut {
        mods,
        keys: vec![Hotkey::Vk(two)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_hotkey_is_shift_2_with_command_or_ctrl() {
        let key = default_hotkey();
        assert_eq!(Shortcut::parse(&key.name()), Some(key.clone()));
        assert!(key.label().ends_with("+2"), "{}", key.label());
        assert!(key.mods.contains(Mods::SHIFT));
        assert_ne!(key, Shortcut::default());
    }
}
