//! Whisper model files (whisper.cpp GGML format) and downloading them.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub file: &'static str,
    pub size_mb: u32,
}

/// All models are multilingual, so speech in any language is written as spoken.
pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "large-v3-turbo-q5_0",
        label: "Large v3 Turbo (recommended)",
        file: "ggml-large-v3-turbo-q5_0.bin",
        size_mb: 547,
    },
    ModelInfo {
        id: "large-v3-turbo",
        label: "Large v3 Turbo, full precision",
        file: "ggml-large-v3-turbo.bin",
        size_mb: 1550,
    },
    ModelInfo {
        id: "medium-q5_0",
        label: "Medium (smaller, slower than Turbo)",
        file: "ggml-medium-q5_0.bin",
        size_mb: 514,
    },
    ModelInfo {
        id: "small",
        label: "Small (fast, less accurate)",
        file: "ggml-small.bin",
        size_mb: 466,
    },
    ModelInfo {
        id: "base",
        label: "Base (fastest, for testing)",
        file: "ggml-base.bin",
        size_mb: 142,
    },
];

pub const DEFAULT_MODEL: ModelInfo = MODELS[0];

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

impl ModelInfo {
    pub fn by_id(id: &str) -> Option<ModelInfo> {
        MODELS.iter().copied().find(|m| m.id == id)
    }

    pub fn path(&self) -> PathBuf {
        models_dir().join(self.file)
    }

    pub fn is_downloaded(&self) -> bool {
        self.path().is_file()
    }

    pub fn url(&self) -> String {
        format!("{BASE_URL}/{}", self.file)
    }
}

/// `~/Library/Application Support/Meeting Transcriber/models` on macOS.
/// Set MEETING_TRANSCRIBER_MODELS to use another folder.
pub fn models_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MEETING_TRANSCRIBER_MODELS") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Meeting Transcriber")
        .join("models")
}

/// Downloads a model, reporting (bytes done, bytes total). Writes to a
/// `.part` file first so an interrupted download is never mistaken for a model.
pub fn download(
    model: ModelInfo,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    let target = model.path();
    let dir = target.parent().context("model path has no folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let part = target.with_extension("bin.part");

    let response = ureq::get(&model.url())
        .call()
        .with_context(|| format!("could not download {}", model.url()))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = response.into_body().into_reader();
    let mut file = File::create(&part).with_context(|| format!("could not write {}", part.display()))?;

    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            bail!("download cancelled");
        }
        let n = reader.read(&mut buf).context("download interrupted")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("could not write model file")?;
        done += n as u64;
        progress(done, total);
    }
    file.sync_all().ok();
    drop(file);

    if let Some(total) = total
        && done != total
    {
        let _ = std::fs::remove_file(&part);
        bail!("download incomplete ({done} of {total} bytes)");
    }
    std::fs::rename(&part, &target).context("could not move the downloaded model into place")?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_table_is_consistent() {
        for m in MODELS {
            assert_eq!(ModelInfo::by_id(m.id), Some(*m));
            assert!(m.file.starts_with("ggml-") && m.file.ends_with(".bin"));
            assert!(m.url().ends_with(m.file));
            // English-only ".en" models would break mixed-language meetings.
            assert!(!m.file.contains(".en"));
        }
        assert_eq!(ModelInfo::by_id("nope"), None);
    }
}
