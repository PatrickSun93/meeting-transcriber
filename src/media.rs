//! Reads the sound of an audio or video file (a Teams or Zoom recording, a
//! voice memo) as 16 kHz mono, with Symphonia: pure Rust, nothing to install.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, bail};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::audio::dsp::{Resampler, SAMPLE_RATE, interleaved_to_mono};

/// File types offered in the file dialog. Others are still tried when
/// dropped on the window.
pub const EXTENSIONS: &[&str] = &[
    "mp4", "m4a", "mov", "m4v", "mp3", "wav", "flac", "mkv", "aac", "ogg", "aiff", "aif", "caf",
];

/// Whether a dropped file looks like something to transcribe.
pub fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The first audio track of `path` as 16 kHz mono. `progress` gets the
/// seconds read so far; `cancel` stops early with an error.
pub fn decode(path: &Path, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<Vec<f32>> {
    let file = std::fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
        .with_context(|| format!("{} is not an audio or video file this app can read", path.display()))?;
    let mut format = probed.format;
    // A video's first track is usually the picture.
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .with_context(|| format!("{} has no sound", path.display()))?;
    let track_id = track.id;
    let rate = track.codec_params.sample_rate.unwrap_or(SAMPLE_RATE);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .with_context(|| {
            format!(
                "the sound in {} is in a format this app can't read (Opus, as in WebM, is one): \
                 convert it to MP4, M4A or MP3 first",
                path.display()
            )
        })?;

    let mut resampler = Resampler::new(rate);
    let mut out = Vec::new();
    let mut buffer: Option<SampleBuffer<f32>> = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            // A new stream starts (chained Ogg): what came before is kept.
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // One damaged packet costs a moment of sound, not the file.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e).with_context(|| format!("could not decode {}", path.display())),
        };
        let spec = *decoded.spec();
        let samples = match &mut buffer {
            Some(b) if b.capacity() >= decoded.capacity() * spec.channels.count() => b,
            slot => slot.insert(SampleBuffer::new(decoded.capacity() as u64, spec)),
        };
        samples.copy_interleaved_ref(decoded);
        let mono = interleaved_to_mono(samples.samples(), spec.channels.count());
        out.extend(resampler.process(&mono));
        progress(out.len() as f32 / SAMPLE_RATE as f32);
    }
    if out.is_empty() {
        bail!("{} has no sound", path.display());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_files_are_recognised_by_extension() {
        assert!(is_media(Path::new("Recording.MP4")));
        assert!(is_media(Path::new("/a/b/memo.m4a")));
        assert!(!is_media(Path::new("notes.md")));
        assert!(!is_media(Path::new("no-extension")));
    }

    #[test]
    fn a_wav_file_decodes_to_16_khz_mono() {
        let dir = std::env::temp_dir().join(format!("mt-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..48_000 {
            let v = ((i as f32 / 48.0).sin() * 8000.0) as i16;
            w.write_sample(v).unwrap();
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        let samples = decode(&path, &AtomicBool::new(false), |_| {}).unwrap();
        // One second, give or take the resampler's edge.
        assert!((15_900..=16_100).contains(&samples.len()), "{}", samples.len());
        assert!(samples.iter().any(|s| s.abs() > 0.1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_media_says_so() {
        let dir = std::env::temp_dir().join(format!("mt-media-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake.mp4");
        std::fs::write(&path, b"not a video").unwrap();
        let err = decode(&path, &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("not an audio or video file"), "{err:#}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
