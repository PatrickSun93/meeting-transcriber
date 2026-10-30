//! Audio capture. Every source delivers 16 kHz mono f32 packets on a channel.

pub mod dsp;
mod file;
mod mic;
mod system;

use crossbeam_channel::Sender;

pub use file::{fake_source, start_file};
pub use mic::start_mic;
pub use system::start_system_audio;

use crate::transcript::Speaker;

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
    fn new(speaker: Speaker, sample_rate: u32, channels: usize, tx: Sender<AudioPacket>) -> Self {
        Self {
            speaker,
            channels,
            resampler: dsp::Resampler::new(sample_rate),
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
