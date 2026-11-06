//! A scrolling screenshot: an area of a display taken again and again
//! while its content scrolls (by itself with the mouse wheel, or by hand),
//! each picture stitched onto the last (`stitch.rs`), until the content
//! stops moving at the end of the page, the picture reaches
//! [`MAX_HEIGHT`], or the user stops it. Each system's backend reads the
//! area and turns the wheel (`Region` in `macos.rs`, `windows.rs`,
//! `linux.rs`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;

use super::Display;
use super::image::Image;
use super::imp::Region;
use super::select::PxRect;
use super::stitch::{Step, Stitcher};

/// The longest a scrolling screenshot gets, in pixels.
pub const MAX_HEIGHT: u32 = 30_000;
/// The shortest area that can be scrolled, in pixels: too few rows to
/// compare below this.
pub const MIN_HEIGHT: u32 = 48;
/// For the overlay to leave the screen before the first picture.
const START_WAIT: Duration = Duration::from_millis(250);
/// For the content to move after a turn of the wheel.
const SETTLE: Duration = Duration::from_millis(180);
/// Between pictures while the user scrolls by hand.
const POLL: Duration = Duration::from_millis(120);
/// The content has not moved after this many turns of the wheel: the end.
const STILL_STEPS: u32 = 4;

/// The part of a display a scrolling screenshot takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub display: Display,
    /// The display's size in pixels.
    pub display_size: [u32; 2],
    /// In the display's pixels, as in its screenshot.
    pub rect: PxRect,
}

impl Area {
    /// Left, top, right and bottom on the desktop, in its pixels.
    pub fn desktop(&self) -> [i32; 4] {
        let [x, y] = self.display.origin_px;
        let (left, top) = (x + self.rect.x as i32, y + self.rect.y as i32);
        [left, top, left + self.rect.width as i32, top + self.rect.height as i32]
    }

    /// The middle of the area on the desktop.
    pub fn center(&self) -> [i32; 2] {
        let [l, t, r, b] = self.desktop();
        [(l + r) / 2, (t + b) / 2]
    }

    fn contains(&self, [x, y]: [i32; 2]) -> bool {
        let [l, t, r, b] = self.desktop();
        (l..r).contains(&x) && (t..b).contains(&y)
    }

    /// Four bars `thickness` pixels thick just outside the area, to show
    /// it without being in its pictures: top, bottom, left, right, each as
    /// left, top, width and height on the desktop.
    pub fn outline(&self, thickness: i32) -> [[i32; 4]; 4] {
        let [l, t, r, b] = self.desktop();
        let t2 = thickness;
        [
            [l - t2, t - t2, r - l + 2 * t2, t2],
            [l - t2, b, r - l + 2 * t2, t2],
            [l - t2, t, t2, b - t],
            [r, t, t2, b - t],
        ]
    }
}

/// What the scrolling is doing, for the window to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Turning the wheel by itself.
    Scrolling,
    /// Waiting for the pointer to come back over the area.
    Paused,
    /// The user scrolls; the app may not (the reason).
    ByHand(&'static str),
    /// The last picture did not fit on: moved too far, or back up.
    Lost,
}

/// How far it has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Of the picture so far, in pixels.
    pub height: u32,
    pub state: State,
}

/// Why it stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Asked to.
    Stopped,
    /// The content stopped moving: the end of the page.
    Bottom,
    /// The picture reached `MAX_HEIGHT`.
    Limit,
}

/// A finished scrolling screenshot.
pub struct Long {
    pub image: Image,
    pub end: End,
}

/// Takes a scrolling screenshot of `area` until `stop` is set or it ends,
/// telling `progress` after each picture. Blocking: called on a thread of
/// its own. Fails when the area cannot be read (permission, Wayland).
pub fn run(area: Area, stop: &AtomicBool, mut progress: impl FnMut(Progress)) -> Result<Long> {
    std::thread::sleep(START_WAIT);
    let mut region = Region::new(&area)?;
    let mut stitcher = Stitcher::new(region.grab()?);
    let mut by_hand = region.cannot_scroll();
    if by_hand.is_none() {
        region.move_pointer(area.center());
    }
    // How far each turn of the wheel asks to go: half the area at first.
    let mut distance = (area.rect.height / 2).max(1);
    let mut still = 0;
    let end = loop {
        if stop.load(Ordering::Relaxed) {
            break End::Stopped;
        }
        let over = region.pointer().is_some_and(|p| area.contains(p));
        let mut turned = false;
        if by_hand.is_none() && over {
            match region.scroll(distance) {
                Ok(()) => turned = true,
                Err(why) => by_hand = Some(why),
            }
        }
        std::thread::sleep(if turned { SETTLE } else { POLL });
        let step = stitcher.push(region.grab()?);
        match step {
            Step::Grew(_) => still = 0,
            Step::Still if turned => still += 1,
            Step::Still => {}
            // Too far for the pictures to overlap: smaller turns.
            Step::Lost if turned => distance = (distance / 2).max(1),
            Step::Lost => {}
        }
        let state = match (step, by_hand) {
            (Step::Lost, _) => State::Lost,
            (_, Some(why)) => State::ByHand(why),
            _ if over => State::Scrolling,
            _ => State::Paused,
        };
        progress(Progress {
            height: stitcher.height().min(MAX_HEIGHT),
            state,
        });
        if stitcher.height() >= MAX_HEIGHT {
            break End::Limit;
        }
        if still >= STILL_STEPS {
            break End::Bottom;
        }
    };
    Ok(Long {
        image: stitcher.finish(MAX_HEIGHT),
        end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Area {
        Area {
            display: Display {
                origin_px: [-1920, 100],
                scale: 1.0,
            },
            display_size: [1920, 1080],
            rect: PxRect {
                x: 100,
                y: 50,
                width: 400,
                height: 300,
            },
        }
    }

    #[test]
    fn the_area_on_the_desktop() {
        let a = area();
        assert_eq!(a.desktop(), [-1820, 150, -1420, 450]);
        assert_eq!(a.center(), [-1620, 300]);
        assert!(a.contains([-1820, 150]));
        assert!(!a.contains([-1420, 300]), "the right edge is outside");
    }

    #[test]
    fn the_outline_goes_around_the_area() {
        let [top, bottom, left, right] = area().outline(3);
        assert_eq!(top, [-1823, 147, 406, 3]);
        assert_eq!(bottom, [-1823, 450, 406, 3]);
        assert_eq!(left, [-1823, 150, 3, 300]);
        assert_eq!(right, [-1420, 150, 3, 300]);
    }
}
