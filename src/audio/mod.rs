//! Audio capture. Every source delivers 16 kHz mono f32 packets on a channel.

pub mod dsp;
mod file;
mod mic;
mod silence;
mod system;

use crossbeam_channel::Sender;

pub use file::{fake_source, start_file};
pub use mic::start_mic;
pub use system::start_system_audio;

use crate::transcript::Speaker;

/// The microphone (as `Speaker::Me`) or the PC's sound (`Others`) as mono
/// at `rate` frames per second: the sound of a screen recording on
/// Windows. (The Mac's comes with the screen, through ScreenCaptureKit.)
#[cfg(windows)]
pub fn start_at(speaker: Speaker, rate: u32, tx: Sender<AudioPacket>) -> anyhow::Result<Capture> {
    let input = match speaker {
        Speaker::Me => mic::Input::Microphone,
        Speaker::Others => mic::Input::Loopback,
    };
    mic::start(input, rate, tx)
}

pub struct AudioPacket {
    pub speaker: Speaker,
    pub samples: Vec<f32>,
}

/// A running capture. Dropping it stops the capture.
pub struct Capture {
    /// Human-readable source name, e.g. the microphone's device name.
    pub name: String,
    _guard: Box<dyn Send>,
}

impl Capture {
    fn new(name: String, guard: impl Send + 'static) -> Self {
        Self {
            name,
            _guard: Box::new(guard),
        }
    }
}

/// Converts a source's native audio to 16 kHz mono and sends it on.
struct Converter {
    speaker: Speaker,
    channels: usize,
    resampler: dsp::Resampler,
    tx: Sender<AudioPacket>,
}

impl Converter {
    /// To 16 kHz: the Mac's and Linux's system sound. On Windows every
    /// source is a cpal stream (`mic.rs`), which says its rate (`to_rate`).
    #[cfg(not(windows))]
    fn new(speaker: Speaker, sample_rate: u32, channels: usize, tx: Sender<AudioPacket>) -> Self {
        Self::to_rate(speaker, sample_rate, channels, dsp::SAMPLE_RATE, tx)
    }

    /// Mono at `out_rate` instead of 16 kHz.
    fn to_rate(speaker: Speaker, sample_rate: u32, channels: usize, out_rate: u32, tx: Sender<AudioPacket>) -> Self {
        Self {
            speaker,
            channels,
            resampler: dsp::Resampler::to(sample_rate, out_rate),
            tx,
        }
    }

    fn push_interleaved(&mut self, data: &[f32]) {
        let mono = dsp::interleaved_to_mono(data, self.channels);
        self.push_mono(&mono);
    }

    fn push_mono(&mut self, mono: &[f32]) {
        let samples = self.resampler.process(mono);
        if !samples.is_empty() {
            // The receiver is gone only while shutting down.
            let _ = self.tx.send(AudioPacket {
                speaker: self.speaker,
                samples,
            });
        }
    }
}
