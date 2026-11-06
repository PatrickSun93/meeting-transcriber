//! Screen recording: an area of a display recorded into an MP4 file
//! (H.264, at about 30 frames a second, with the pointer), with the
//! computer's sound and the microphone, until it is stopped. Each system
//! records in its own way: the Mac's ScreenCaptureKit writes the file
//! itself (`macos.rs`); on Windows the area is copied with GDI and Media
//! Foundation encodes it (`windows.rs`), its sound mixed here first
//! (`mixer.rs`, `nv12.rs` for the pictures). Linux has none yet.
//! The sizes, the frame timing and the timer's text are here, so their
//! tests run everywhere.

#[cfg(any(windows, test))]
pub mod mixer;
#[cfg(any(windows, test))]
pub mod nv12;

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

use std::path::Path;
#[cfg(any(windows, test))]
use std::time::Duration;

use anyhow::{Result, bail};

use super::scrolling::Area;

/// Frames a second.
pub const FPS: u32 = 30;
/// The smallest area that can be recorded, in pixels each way.
pub const MIN_SIDE: u32 = 32;
/// The largest video, long side and short side: 4K, which every H.264
/// encoder takes. A larger area is scaled down to fit.
const MAX_SIZE: [u32; 2] = [3840, 2160];

/// What a recording hears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sound {
    /// Everything the computer plays.
    pub computer: bool,
    pub microphone: bool,
}

impl Sound {
    pub fn any(self) -> bool {
        self.computer || self.microphone
    }
}

/// A finished recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recorded {
    /// How long it is.
    pub seconds: f64,
}

/// A recording going on. Dropping it without `stop` may leave the file
/// unfinished.
pub struct Recorder(imp::Recorder);

impl Recorder {
    /// Why it stopped by itself (a full disk, the permission taken away), if it did.
    pub fn failure(&self) -> Option<String> {
        self.0.failure()
    }

    /// Stops and finishes the file. Blocking (up to a few seconds).
    pub fn stop(self) -> Result<Recorded> {
        self.0.stop()
    }
}

/// Why the screen cannot be recorded here, if it cannot: Linux, or a Mac
/// before macOS 15.
pub fn unsupported() -> Option<&'static str> {
    imp::unsupported()
}

/// Starts recording `area` into the MP4 file `path`, hearing `sound`.
/// Blocking (up to a few seconds): called on a thread of its own. Fails
/// without permission, or where recording is not supported, saying so.
pub fn start(area: &Area, path: &Path, sound: Sound) -> Result<Recorder> {
    if let Some(why) = unsupported() {
        bail!("{why}");
    }
    if area.rect.width < MIN_SIDE || area.rect.height < MIN_SIDE {
        bail!("the area is too small to record");
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| anyhow::anyhow!("could not create {}: {e}", dir.display()))?;
    }
    imp::Recorder::start(area, path, sound).map(Recorder)
}

/// The video's size for an area of `width` × `height` pixels: the same,
/// or scaled down to fit 4K, and even both ways (as H.264's colour, at
/// half the size, needs).
pub fn video_size(width: u32, height: u32) -> [u32; 2] {
    let (long, short) = (width.max(height).max(1), width.min(height).max(1));
    let fit = (f64::from(MAX_SIZE[0]) / f64::from(long))
        .min(f64::from(MAX_SIZE[1]) / f64::from(short))
        .min(1.0);
    let even = |v: u32| (((f64::from(v) * fit).floor() as u32) & !1).max(2);
    [even(width), even(height)]
}

/// How many bits a second the video is encoded at: enough for sharp text
/// on a screen, about 5 Mbit/s at 1080p.
#[cfg(any(windows, test))]
pub fn bitrate([width, height]: [u32; 2]) -> u32 {
    let bits = f64::from(width) * f64::from(height) * f64::from(FPS) * 0.08;
    bits.clamp(1_000_000.0, 24_000_000.0) as u32
}

/// When a recording's frames are taken: `fps` a second by
/// the clock. A frame that comes late skips the ones it missed rather than
/// catching up, and each is stamped with when it was taken, so a slow
/// computer makes the video jerkier, never faster.
#[cfg(any(windows, test))]
pub struct FrameClock {
    interval: Duration,
    /// When the next frame is due, from the start.
    next: Duration,
}

#[cfg(any(windows, test))]
impl FrameClock {
    pub fn new(fps: u32) -> Self {
        Self {
            interval: Duration::from_secs(1) / fps.max(1),
            next: Duration::ZERO,
        }
    }

    /// How long a frame lasts.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// A frame was taken, and it is now `now` from the start: how long to
    /// wait for the next one.
    pub fn wait_after(&mut self, now: Duration) -> Duration {
        self.next += self.interval;
        if self.next <= now {
            let missed = (now - self.next).as_nanos() / self.interval.as_nanos() + 1;
            self.next += self.interval * missed as u32;
        }
        self.next - now
    }
}

/// A recording's length as its timer shows it: "0:07", "12:03", "1:02:03".
pub fn timer(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_video_is_even_and_at_most_4k() {
        assert_eq!(video_size(1920, 1080), [1920, 1080]);
        assert_eq!(video_size(801, 451), [800, 450]);
        // A Retina 5K display, an ultrawide and a portrait one fit in 4K.
        assert_eq!(video_size(5120, 2880), [3840, 2160]);
        assert_eq!(video_size(5120, 1440), [3840, 1080]);
        assert_eq!(video_size(2880, 5120), [2160, 3840]);
        assert_eq!(video_size(1440, 2560), [1440, 2560]);
        assert_eq!(video_size(0, 0), [2, 2]);
        for (w, h) in [(33, 999), (4097, 33), (6016, 3384)] {
            let [vw, vh] = video_size(w, h);
            assert!(vw % 2 == 0 && vh % 2 == 0 && vw.max(vh) <= 3840 && vw.min(vh) <= 2160);
        }
    }

    #[test]
    fn the_bitrate_follows_the_size() {
        assert_eq!(bitrate([1920, 1080]), 4_976_640);
        assert_eq!(bitrate([64, 64]), 1_000_000);
        assert_eq!(bitrate([3840, 2160]), 19_906_560);
    }

    #[test]
    fn frames_keep_to_the_clock_and_late_ones_skip() {
        let ms = Duration::from_millis;
        let mut clock = FrameClock::new(FPS);
        let frame = clock.interval();
        assert_eq!(frame, Duration::from_nanos(33_333_333));
        // On time: the next frame is due one interval after the start.
        assert_eq!(clock.wait_after(ms(5)), frame - ms(5));
        // Taken on time at 33 ms: wait for 66 ms.
        assert_eq!(clock.wait_after(frame), frame);
        // A frame that took 100 ms skips the frames it missed (at 100 ms
        // and before), rather than taking them in a hurry.
        let now = frame * 2 + ms(100);
        let wait = clock.wait_after(now);
        assert!(wait > Duration::ZERO && wait <= frame, "{wait:?}");
        assert_eq!((now + wait).as_nanos() % frame.as_nanos(), 0, "on the grid");
        // Exactly on a due time: that one is missed too, the next is a whole frame away.
        let mut clock = FrameClock::new(FPS);
        assert_eq!(clock.wait_after(frame), frame);
    }

    #[test]
    fn the_timer_reads_like_a_clock() {
        assert_eq!(timer(7), "0:07");
        assert_eq!(timer(723), "12:03");
        assert_eq!(timer(3723), "1:02:03");
    }

    #[test]
    fn sound_is_any_of_the_two() {
        let none = Sound {
            computer: false,
            microphone: false,
        };
        assert!(!none.any());
        assert!(
            Sound {
                microphone: true,
                ..none
            }
            .any()
        );
    }
}
