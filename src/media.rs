//! Reads the sound of an audio or video file (a Teams or Zoom recording, a
//! voice memo) as 16 kHz mono, with Symphonia: pure Rust, nothing to install.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, bail};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::{FormatOptions, FormatReader, Track};
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

/// The sound of `path` as 16 kHz mono: every audio track, mixed (a
/// screen recording on the Mac has the computer's sound and the
/// microphone on tracks of their own). `progress` gets the seconds read so
/// far; `cancel` stops early with an error.
pub fn decode(path: &Path, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<Vec<f32>> {
    let mut format = open(path)?;
    let mut tracks = Vec::new();
    let mut unreadable = None;
    for track in format.tracks().iter().filter(|t| is_sound(t)) {
        match symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()) {
            Ok(decoder) => tracks.push(Sound {
                id: track.id,
                decoder,
                resampler: Resampler::new(track.codec_params.sample_rate.unwrap_or(SAMPLE_RATE)),
                buffer: None,
                out: Vec::new(),
            }),
            Err(e) => unreadable = Some(e),
        }
    }
    if tracks.is_empty() {
        return match unreadable {
            Some(e) => Err(e).with_context(|| {
                format!(
                    "the sound in {} is in a format this app can't read (Opus, as in WebM, is one): \
                     convert it to MP4, M4A or MP3 first",
                    path.display()
                )
            }),
            None => bail!("{} has no sound", path.display()),
        };
    }

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
        let Some(track) = tracks.iter_mut().find(|t| t.id == packet.track_id()) else {
            continue;
        };
        let decoded = match track.decoder.decode(&packet) {
            Ok(d) => d,
            // One damaged packet costs a moment of sound, not the file.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e).with_context(|| format!("could not decode {}", path.display())),
        };
        let spec = *decoded.spec();
        let samples = match &mut track.buffer {
            Some(b) if b.capacity() >= decoded.capacity() * spec.channels.count() => b,
            slot => slot.insert(SampleBuffer::new(decoded.capacity() as u64, spec)),
        };
        samples.copy_interleaved_ref(decoded);
        let mono = interleaved_to_mono(samples.samples(), spec.channels.count());
        track.out.extend(track.resampler.process(&mono));
        progress(track.out.len() as f32 / SAMPLE_RATE as f32);
    }
    let out = tracks.into_iter().map(|t| t.out).reduce(mix).unwrap_or_default();
    if out.is_empty() {
        bail!("{} has no sound", path.display());
    }
    Ok(out)
}

/// Whether `path` has a track of sound, as a screen recording does when
/// it was made with sound.
pub fn has_sound(path: &Path) -> bool {
    open(path).is_ok_and(|format| format.tracks().iter().any(is_sound))
}

/// One audio track being read.
struct Sound {
    id: u32,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    resampler: Resampler,
    buffer: Option<SampleBuffer<f32>>,
    /// What it has given so far, at 16 kHz.
    out: Vec<f32>,
}

fn open(path: &Path) -> Result<Box<dyn FormatReader>> {
    let file = std::fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
        .with_context(|| format!("{} is not an audio or video file this app can read", path.display()))?;
    Ok(probed.format)
}

/// Whether a track is sound: a video's first track is usually the picture.
fn is_sound(track: &Track) -> bool {
    track.codec_params.codec != CODEC_TYPE_NULL && track.codec_params.sample_rate.is_some()
}

/// Two tracks of the same timeline played together: added up, in range.
fn mix(mut a: Vec<f32>, b: Vec<f32>) -> Vec<f32> {
    if a.len() < b.len() {
        a.resize(b.len(), 0.0);
    }
    for (x, y) in a.iter_mut().zip(b) {
        *x = (*x + y).clamp(-1.0, 1.0);
    }
    a
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
        assert!(has_sound(&path));
        assert!(!has_sound(&dir.join("missing.wav")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tracks_are_mixed_on_one_timeline() {
        assert_eq!(mix(vec![0.5, 0.75], vec![0.25, 0.5, -0.5]), vec![0.75, 1.0, -0.5]);
        assert_eq!(mix(vec![0.1, 0.2, 0.3], vec![-0.1]), vec![0.0, 0.2, 0.3]);
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
