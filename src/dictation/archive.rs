//! The recordings archive: each dictation as a 16 kHz mono WAV with its text
//! in a `.txt` of the same name. On the Mac it is the folder voice2cursor
//! used, so the history it built up carries on here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use chrono::{NaiveDateTime, Timelike as _};

use crate::audio::dsp::SAMPLE_RATE;

/// voice2cursor's folder on the Mac; elsewhere, where voice2cursor never
/// ran, `Dictation` in the app data folder.
pub fn default_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".voice2cursor")
            .join("recordings")
    } else {
        crate::models::app_support_dir().join("Dictation")
    }
}

/// `YYYYMMDD_HHMMSS`, then `_2`, `_3`, … for more clips in the same second
/// (voice2cursor's naming). `taken` says whether a stem is in use.
pub fn clip_stem(now: NaiveDateTime, taken: impl Fn(&str) -> bool) -> String {
    let stem = now.format("%Y%m%d_%H%M%S").to_string();
    if !taken(&stem) {
        return stem;
    }
    (2..)
        .map(|n| format!("{stem}_{n}"))
        .find(|s| !taken(s))
        .expect("an unused name")
}

/// When a clip was recorded, from its file name: `20261029_230021[_n]` as
/// written here and by voice2cursor, or `20261029-230021-921` as written by
/// its cross-platform version.
pub fn parse_stem(stem: &str) -> Option<NaiveDateTime> {
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    let (date, rest) = (stem.get(..8)?, stem.get(8..)?);
    let (sep, rest) = (rest.get(..1)?, rest.get(1..)?);
    let time = rest.get(..6)?;
    let tail = rest.get(6..)?;
    if !digits(date, 8) || !digits(time, 6) {
        return None;
    }
    let millis = match (sep, tail) {
        ("_", "") => 0,
        ("_", t) if t.len() > 1 && t.starts_with('_') && digits(&t[1..], t.len() - 1) => 0,
        ("-", t) if t.len() == 4 && t.starts_with('-') && digits(&t[1..], 3) => t[1..].parse().ok()?,
        _ => return None,
    };
    let at = NaiveDateTime::parse_from_str(&format!("{date}{time}"), "%Y%m%d%H%M%S").ok()?;
    at.with_nanosecond(millis * 1_000_000)
}

/// Writes 16 kHz mono samples as 16-bit PCM.
pub fn write_wav(path: &Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).with_context(|| format!("could not write {}", path.display()))?;
    for s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    writer.finalize().context("could not finish the recording file")?;
    Ok(())
}

/// Saves a clip under a fresh name and returns the WAV's path.
pub fn save_clip(dir: &Path, now: NaiveDateTime, samples: &[f32]) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let stem = clip_stem(now, |s| {
        dir.join(format!("{s}.wav")).exists() || dir.join(format!("{s}.txt")).exists()
    });
    let path = dir.join(format!("{stem}.wav"));
    write_wav(&path, samples)?;
    Ok(path)
}

/// Writes a clip's text next to its WAV.
pub fn save_text(wav: &Path, text: &str) -> Result<()> {
    let path = wav.with_extension("txt");
    std::fs::write(&path, format!("{}\n", text.trim_end()))
        .with_context(|| format!("could not write {}", path.display()))
}

/// One dictation in the folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub at: NaiveDateTime,
    pub stem: String,
    pub wav: Option<PathBuf>,
    pub text: String,
    /// Length of the recording, from the WAV's size.
    pub seconds: Option<f32>,
}

impl Clip {
    pub fn matches(&self, query: &str) -> bool {
        query.is_empty() || self.text.to_lowercase().contains(&query.to_lowercase())
    }
}

/// Every clip in `dir`, newest first. A WAV without text is a clip that
/// was not transcribed (nothing was recognized, or it failed).
pub fn list(dir: &Path) -> Vec<Clip> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut clips: BTreeMap<String, Clip> = BTreeMap::new();
    for entry in read.flatten() {
        let path = entry.path();
        let (Some(stem), Some(ext)) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|e| e.to_str()),
        ) else {
            continue;
        };
        let Some(at) = parse_stem(stem) else { continue };
        let clip = clips.entry(stem.to_string()).or_insert_with(|| Clip {
            at,
            stem: stem.to_string(),
            wav: None,
            text: String::new(),
            seconds: None,
        });
        match ext {
            "wav" => {
                // 44-byte header, then 2 bytes a sample.
                clip.seconds = entry
                    .metadata()
                    .ok()
                    .map(|m| m.len().saturating_sub(44) as f32 / (2 * SAMPLE_RATE) as f32);
                clip.wav = Some(path);
            }
            "txt" => clip.text = std::fs::read_to_string(&path).unwrap_or_default().trim().to_string(),
            _ => {}
        }
    }
    let mut clips: Vec<Clip> = clips.into_values().collect();
    clips.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.stem.cmp(&a.stem)));
    clips
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").unwrap()
    }

    #[test]
    fn names_follow_voice2cursor() {
        let now = at("2026-10-30 09:05:07.250");
        assert_eq!(clip_stem(now, |_| false), "20261030_090507");
        let taken = ["20261030_090507", "20261030_090507_2"];
        assert_eq!(clip_stem(now, |s| taken.contains(&s)), "20261030_090507_3");
    }

    #[test]
    fn both_archive_namings_are_read() {
        assert_eq!(parse_stem("20260901_222745"), Some(at("2026-09-01 22:27:45.0")));
        assert_eq!(parse_stem("20260901_222745_2"), Some(at("2026-09-01 22:27:45.0")));
        assert_eq!(parse_stem("20261029-230021-921"), Some(at("2026-10-29 23:00:21.921")));
        for bad in [
            "",
            "20261029",
            "20261029_2300",
            "20261029_230021_",
            "20261029_230021x",
            "20261029-230021",
            "20261029-230021-92",
            "20261399_230021",
            "notes",
        ] {
            assert_eq!(parse_stem(bad), None, "{bad}");
        }
    }

    #[test]
    fn saved_clips_are_listed_newest_first() {
        let dir = std::env::temp_dir().join(format!("meeting-transcriber-test-{}-archive", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let now = at("2026-10-30 09:05:07.0");
        let first = save_clip(&dir, now, &[0.5; 16_000]).unwrap();
        let second = save_clip(&dir, now, &[0.0; 8_000]).unwrap();
        assert_eq!(first.file_name().unwrap(), "20261030_090507.wav");
        assert_eq!(second.file_name().unwrap(), "20261030_090507_2.wav");
        save_text(&first, "Hello there").unwrap();
        std::fs::write(dir.join("20261029-230021-921.txt"), "旧的\n").unwrap();
        std::fs::write(dir.join("readme.txt"), "not a clip").unwrap();

        let clips = list(&dir);
        let stems: Vec<&str> = clips.iter().map(|c| c.stem.as_str()).collect();
        assert_eq!(stems, ["20261030_090507_2", "20261030_090507", "20261029-230021-921"]);
        assert_eq!(clips[1].text, "Hello there");
        assert!((clips[1].seconds.unwrap() - 1.0).abs() < 0.01);
        assert_eq!(clips[0].text, "");
        assert_eq!(clips[2].wav, None);
        assert!(clips[2].matches("旧"));
        assert!(clips[1].matches("hello"));
        assert!(!clips[1].matches("bye"));

        let reader = hound::WavReader::open(&first).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().bits_per_sample, 16);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
