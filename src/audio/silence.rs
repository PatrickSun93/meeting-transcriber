//! Keeps a source's timeline in step with the clock when it goes quiet.
//!
//! A meeting line's time is its source's first packet plus the samples
//! counted since, and the chunker finds the end of an utterance by hearing
//! silence. Loopback captures (Windows WASAPI, and a sound server's monitor
//! on Linux) may send nothing at all while nothing plays, which would stall
//! the line in progress and pull every later timestamp early. So a source
//! that has sent nothing for a while is topped up with real silence.

// Only the loopback captures on Windows and Linux fill gaps; macOS's
// ScreenCaptureKit and every microphone deliver continuously.
#![cfg_attr(target_os = "macos", allow(dead_code))]

use std::time::{Duration, Instant};

use super::Converter;

/// Quiet this long counts as a gap. Well above any device period, so a
/// source that sends large buffers is never padded mid-speech.
const GAP: Duration = Duration::from_millis(500);

/// How often a capture should call `Paced::tick`.
pub const TICK: Duration = Duration::from_millis(100);

/// Counts a source's frames against the clock.
pub struct SilenceFill {
    rate: u32,
    started: Instant,
    frames: u64,
    last_data: Instant,
}

impl SilenceFill {
    /// `rate`: the source's frames per second.
    pub fn new(rate: u32, now: Instant) -> Self {
        Self {
            rate,
            started: now,
            frames: 0,
            last_data: now,
        }
    }

    /// Notes `frames` frames that just arrived.
    pub fn record(&mut self, frames: usize, now: Instant) {
        self.frames += frames as u64;
        self.last_data = now;
    }

    /// Frames of silence to insert now: none while data is flowing, else
    /// enough to catch up with the clock. Counts them as sent.
    pub fn missing(&mut self, now: Instant) -> usize {
        if now.saturating_duration_since(self.last_data) < GAP {
            return 0;
        }
        let due = (now.saturating_duration_since(self.started).as_secs_f64() * f64::from(self.rate)) as u64;
        let missing = due.saturating_sub(self.frames);
        self.frames += missing;
        missing as usize
    }
}

/// A converter, optionally with gaps filled with silence.
pub struct Paced {
    converter: Converter,
    fill: Option<SilenceFill>,
}

impl Paced {
    /// Passes audio through as it comes.
    pub fn new(converter: Converter) -> Self {
        Self { converter, fill: None }
    }

    /// Fills gaps; `rate` is the source's frame rate.
    pub fn with_silence_fill(converter: Converter, rate: u32) -> Self {
        Self {
            converter,
            fill: Some(SilenceFill::new(rate, Instant::now())),
        }
    }

    pub fn push_interleaved(&mut self, data: &[f32]) {
        if let Some(fill) = &mut self.fill {
            fill.record(data.len() / self.converter.channels.max(1), Instant::now());
        }
        self.converter.push_interleaved(data);
    }

    /// Sends the silence a gap needs, if any.
    pub fn tick(&mut self) {
        let missing = self.fill.as_mut().map_or(0, |f| f.missing(Instant::now()));
        if missing > 0 {
            // Mono at the source rate: the converter only resamples it.
            self.converter.push_mono(&vec![0.0; missing]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flowing_audio_is_never_padded() {
        let t0 = Instant::now();
        let mut fill = SilenceFill::new(1_000, t0);
        // Buffers every 400 ms, each a little short of the clock.
        for i in 1..=10 {
            let now = t0 + Duration::from_millis(400 * i);
            fill.record(390, now);
            assert_eq!(fill.missing(now + Duration::from_millis(300)), 0);
        }
    }

    #[test]
    fn a_gap_is_filled_up_to_the_clock() {
        let t0 = Instant::now();
        let mut fill = SilenceFill::new(1_000, t0);
        fill.record(1_000, t0 + Duration::from_secs(1));
        // 0.4 s quiet: not a gap yet.
        assert_eq!(fill.missing(t0 + Duration::from_millis(1_400)), 0);
        // 2 s in, 1 s quiet: one second of silence.
        assert_eq!(fill.missing(t0 + Duration::from_secs(2)), 1_000);
        // Still quiet a tick later: only the new 100 ms.
        assert_eq!(fill.missing(t0 + Duration::from_millis(2_100)), 100);
        // Audio is back: nothing more.
        fill.record(50, t0 + Duration::from_millis(2_150));
        assert_eq!(fill.missing(t0 + Duration::from_millis(2_200)), 0);
    }

    #[test]
    fn a_source_silent_from_the_start_is_filled() {
        let t0 = Instant::now();
        let mut fill = SilenceFill::new(16_000, t0);
        assert_eq!(fill.missing(t0 + Duration::from_millis(400)), 0);
        assert_eq!(fill.missing(t0 + Duration::from_secs(3)), 48_000);
    }
}
