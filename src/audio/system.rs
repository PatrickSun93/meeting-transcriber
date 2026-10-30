//! System audio capture: everything the Mac plays, i.e. the other people in
//! the meeting. Uses ScreenCaptureKit (macOS 13+), which needs the
//! "Screen & System Audio Recording" permission.

use anyhow::Result;
use crossbeam_channel::Sender;

use super::{AudioPacket, Capture};

#[cfg(target_os = "macos")]
pub fn start_system_audio(tx: Sender<AudioPacket>) -> Result<Capture> {
    mac::start(tx)
}

#[cfg(not(target_os = "macos"))]
pub fn start_system_audio(_tx: Sender<AudioPacket>) -> Result<Capture> {
    anyhow::bail!("capturing system audio is only supported on macOS")
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::Mutex;

    use anyhow::{Context as _, Result};
    use crossbeam_channel::Sender;
    use screencapturekit::prelude::*;

    use super::super::{AudioPacket, Capture, Converter, dsp};
    use crate::transcript::Speaker;

    struct StreamGuard(SCStream);

    impl Drop for StreamGuard {
        fn drop(&mut self) {
            let _ = self.0.stop_capture();
        }
    }

    const PERMISSION_HINT: &str = "allow this app in System Settings > Privacy & Security > \
        Screen & System Audio Recording, then quit and reopen it";

    pub fn start(tx: Sender<AudioPacket>) -> Result<Capture> {
        let content =
            SCShareableContent::get().with_context(|| format!("could not access system audio; {PERMISSION_HINT}"))?;
        let display = content
            .displays()
            .into_iter()
            .next()
            .context("no display found; ScreenCaptureKit needs one to capture audio")?;
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build()
            .context("could not create the capture filter")?;

        // Video cannot be turned off, so ask for the smallest, slowest stream.
        let config = SCStreamConfiguration::new()
            .with_width(2)
            .with_height(2)
            .with_minimum_frame_interval(&CMTime::new(1, 1))
            .with_captures_audio(true)
            .with_excludes_current_process_audio(true)
            .with_sample_rate(dsp::SAMPLE_RATE as i32)
            .with_channel_count(1);

        let converter = Mutex::new(None::<(u32, Converter)>);
        let mut stream = SCStream::new(&filter, &config).context("could not create the capture stream")?;
        stream
            .add_output_handler(
                move |buffer: CMSampleBuffer, kind: SCStreamOutputType| {
                    if kind != SCStreamOutputType::Audio {
                        return;
                    }
                    let Some(mono) = decode_audio(&buffer) else { return };
                    let rate = buffer
                        .format_description()
                        .and_then(|f| f.audio_sample_rate())
                        .map_or(dsp::SAMPLE_RATE, |r| r as u32);
                    let mut slot = converter.lock().unwrap_or_else(|e| e.into_inner());
                    // (Re)build the converter if the stream's rate changes.
                    if slot.as_ref().is_none_or(|(r, _)| *r != rate) {
                        *slot = Some((rate, Converter::new(Speaker::Others, rate, 1, tx.clone())));
                    }
                    if let Some((_, c)) = slot.as_mut() {
                        c.push_mono(&mono);
                    }
                },
                SCStreamOutputType::Audio,
            )
            .context("could not register the audio handler")?;
        stream
            .start_capture()
            .with_context(|| format!("could not start system audio capture; {PERMISSION_HINT}"))?;
        Ok(Capture::new("System audio".into(), StreamGuard(stream)))
    }

    /// ScreenCaptureKit delivers 32-bit float PCM, normally one buffer per
    /// channel. Mixes everything down to mono.
    fn decode_audio(buffer: &CMSampleBuffer) -> Option<Vec<f32>> {
        let list = buffer.audio_buffer_list().ok()?;
        let mut channels: Vec<Vec<f32>> = Vec::new();
        for buf in list.iter() {
            let floats: Vec<f32> = buf
                .data()
                .chunks_exact(4)
                .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            let n = buf.number_channels.max(1) as usize;
            channels.push(dsp::interleaved_to_mono(&floats, n));
        }
        let len = channels.iter().map(Vec::len).min()?;
        let count = channels.len() as f32;
        Some(
            (0..len)
                .map(|i| channels.iter().map(|c| c[i]).sum::<f32>() / count)
                .collect(),
        )
    }
}
