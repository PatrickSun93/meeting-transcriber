//! Plays a WAV file in real time as if it were a live source. For testing the
//! app without a meeting: set MEETING_TRANSCRIBER_FAKE_ME and/or
//! MEETING_TRANSCRIBER_FAKE_OTHERS to WAV file paths.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use crossbeam_channel::Sender;

use super::{AudioPacket, Capture};
use crate::audio::dsp::SAMPLE_RATE;
use crate::transcript::Speaker;

pub fn fake_source(speaker: Speaker) -> Option<PathBuf> {
    let var = match speaker {
        Speaker::Me => "MEETING_TRANSCRIBER_FAKE_ME",
        Speaker::Others => "MEETING_TRANSCRIBER_FAKE_OTHERS",
    };
    std::env::var_os(var).map(PathBuf::from)
}

struct Stop(Option<Sender<()>>);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.take();
    }
}

pub fn start_file(path: PathBuf, speaker: Speaker, tx: Sender<AudioPacket>) -> Result<Capture> {
    let samples = crate::wav::read_wav(&path).with_context(|| format!("fake {} source", speaker.label()))?;
    let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
    std::thread::spawn(move || {
        let packet = (SAMPLE_RATE / 10) as usize;
        let started = Instant::now();
        for (i, chunk) in samples.chunks(packet).enumerate() {
            let due = started + Duration::from_millis(100 * (i as u64 + 1));
            // Returns early when the capture is dropped (sender disconnects).
            if stop_rx.recv_deadline(due).is_err_and(|e| e.is_disconnected()) {
                return;
            }
            let packet = AudioPacket {
                speaker,
                samples: chunk.to_vec(),
            };
            if tx.send(packet).is_err() {
                return;
            }
        }
    });
    let name = format!("{} (file)", path.file_name().unwrap_or_default().to_string_lossy());
    Ok(Capture::new(name, Stop(Some(stop_tx))))
}
