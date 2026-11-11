//! Telling speech from noise before Whisper hears it. Whisper writes
//! something for almost any sound ("Okay.", "Thank you."), so a piece of a
//! meeting in which Silero VAD (run by whisper.cpp) hears too little speech
//! is not transcribed at all.
//!
//! The model (Silero VAD v5.1.2 in whisper.cpp's format, MIT licence,
//! 885 KB) is built into the program and written to the app data folder
//! the first time it is needed.

use std::path::PathBuf;

use anyhow::{Context as _, Result};

pub const MODEL_FILE: &str = "ggml-silero-v5.1.2.bin";
static MODEL: &[u8] = include_bytes!("../assets/ggml-silero-v5.1.2.bin");

/// How sure Silero must be that a moment is speech.
pub const THRESHOLD: f32 = 0.5;
/// Less speech than this in a piece, and it is taken for noise: a cough, a
/// keyboard, breathing, the other side's sound leaking into the microphone.
/// A short real "Yes" still has more.
pub const MIN_SPEECH_SECONDS: f32 = 0.25;

pub struct Vad {
    ctx: whisper_rs::WhisperVadContext,
}

impl Vad {
    pub fn load() -> Result<Self> {
        let path = model_path()?;
        let mut params = whisper_rs::WhisperVadContextParams::new();
        params.set_n_threads(1);
        params.set_use_gpu(false);
        let path = path
            .to_str()
            .context("the speech detection model's path is not valid UTF-8")?;
        let ctx = whisper_rs::WhisperVadContext::new(path, params)
            .map_err(|e| anyhow::anyhow!("could not load the speech detection model {path}: {e}"))?;
        Ok(Self { ctx })
    }

    /// Seconds of `samples` (16 kHz mono) that sound like speech.
    pub fn speech_seconds(&mut self, samples: &[f32]) -> Result<f32> {
        if samples.is_empty() {
            return Ok(0.0);
        }
        self.ctx
            .detect_speech(samples)
            .map_err(|e| anyhow::anyhow!("speech detection failed: {e}"))?;
        let seconds = samples.len() as f32 / crate::audio::dsp::SAMPLE_RATE as f32;
        Ok(speech_seconds(self.ctx.probabilities(), seconds, THRESHOLD))
    }
}

/// The share of `probs` (one per short window, evenly over `seconds`) at
/// or above `threshold`, in seconds.
pub fn speech_seconds(probs: &[f32], seconds: f32, threshold: f32) -> f32 {
    if probs.is_empty() {
        return 0.0;
    }
    let speech = probs.iter().filter(|p| **p >= threshold).count();
    seconds * speech as f32 / probs.len() as f32
}

/// The model's file in the app data folder, written from the program when
/// missing or different.
fn model_path() -> Result<PathBuf> {
    let dir = crate::models::app_support_dir().join("vad");
    let path = dir.join(MODEL_FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == MODEL.len() as u64) {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let tmp = dir.join(format!(".{MODEL_FILE}.{}.saving", std::process::id()));
    std::fs::write(&tmp, MODEL)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_windows_that_sound_like_speech() {
        assert_eq!(speech_seconds(&[], 2.0, 0.5), 0.0);
        assert_eq!(speech_seconds(&[0.1, 0.9, 0.5, 0.2], 2.0, 0.5), 1.0);
        assert_eq!(speech_seconds(&[0.9; 10], 3.0, 0.5), 3.0);
    }

    /// The real model, built into the program: silence and a hum are not
    /// speech.
    #[test]
    fn silence_and_a_hum_are_not_speech() {
        let mut vad = match Vad::load() {
            Ok(vad) => vad,
            // No writable app data folder (a locked-down test machine).
            Err(e) => return eprintln!("skipped: {e:#}"),
        };
        let silence = vec![0.0f32; 32_000];
        assert!(vad.speech_seconds(&silence).unwrap() < MIN_SPEECH_SECONDS);
        let hum: Vec<f32> = (0..32_000)
            .map(|i| 0.05 * (i as f32 * 2.0 * std::f32::consts::PI * 120.0 / 16_000.0).sin())
            .collect();
        assert!(vad.speech_seconds(&hum).unwrap() < MIN_SPEECH_SECONDS);
    }
}
