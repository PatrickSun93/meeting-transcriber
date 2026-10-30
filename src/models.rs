//! Whisper model files (whisper.cpp GGML format) and downloading them.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

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
        id: "large-v3-q5_0",
        label: "Large v3 (more accurate, slower)",
        file: "ggml-large-v3-q5_0.bin",
        size_mb: 1031,
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

/// Overrides the configured models folder, for development and tests.
pub const MODELS_ENV: &str = "MEETING_TRANSCRIBER_MODELS";

/// The folder chosen in Settings; None means the default. Global because
/// every model lookup needs it and it changes rarely (only from Settings).
static CONFIGURED_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

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

    /// Size on disk, or None if it is not downloaded.
    pub fn file_size(&self) -> Option<u64> {
        file_size(&self.path())
    }

    pub fn url(&self) -> String {
        format!("{BASE_URL}/{}", self.file)
    }

    pub fn delete(&self) -> Result<()> {
        let path = self.path();
        std::fs::remove_file(&path).with_context(|| format!("could not delete {}", path.display()))
    }
}

/// `~/Library/Application Support/Meeting Transcriber` on macOS: settings
/// and, by default, the models.
pub fn app_support_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Meeting Transcriber")
}

pub fn default_models_dir() -> PathBuf {
    app_support_dir().join("models")
}

/// The folder set by MEETING_TRANSCRIBER_MODELS, if any.
pub fn env_models_dir() -> Option<PathBuf> {
    std::env::var_os(MODELS_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Sets the folder chosen in Settings (None for the default).
pub fn set_configured_dir(dir: Option<PathBuf>) {
    *CONFIGURED_DIR.write().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// The folder from Settings, or the default, ignoring the environment.
pub fn configured_dir() -> PathBuf {
    resolve_models_dir(None, CONFIGURED_DIR.read().unwrap_or_else(|e| e.into_inner()).clone())
}

/// Where models are read and downloaded: MEETING_TRANSCRIBER_MODELS, else
/// the folder chosen in Settings, else the default.
pub fn models_dir() -> PathBuf {
    resolve_models_dir(
        env_models_dir(),
        CONFIGURED_DIR.read().unwrap_or_else(|e| e.into_inner()).clone(),
    )
}

fn resolve_models_dir(env: Option<PathBuf>, configured: Option<PathBuf>) -> PathBuf {
    env.or(configured).unwrap_or_else(default_models_dir)
}

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// The known models present in `dir`, with their sizes in bytes.
pub fn downloaded_in(dir: &Path) -> Vec<(ModelInfo, u64)> {
    MODELS
        .iter()
        .filter_map(|m| file_size(&dir.join(m.file)).map(|size| (*m, size)))
        .collect()
}

/// Moves every known model from `from` to `to`, reporting (files done,
/// files total). A model already present in `to` is left where it is.
/// Returns how many were moved.
pub fn move_models(from: &Path, to: &Path, mut progress: impl FnMut(usize, usize)) -> Result<usize> {
    let models = downloaded_in(from);
    std::fs::create_dir_all(to).with_context(|| format!("could not create {}", to.display()))?;
    let mut moved = 0;
    for (i, (model, _)) in models.iter().enumerate() {
        progress(i, models.len());
        let (src, dst) = (from.join(model.file), to.join(model.file));
        if dst.exists() {
            continue;
        }
        move_file(&src, &dst).with_context(|| format!("could not move {}", model.file))?;
        moved += 1;
    }
    progress(models.len(), models.len());
    Ok(moved)
}

/// A rename is instant on the same disk; across disks it fails, so copy.
fn move_file(src: &Path, dst: &Path) -> Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_then_remove(src, dst)
}

/// Copies through a `.part` file, so an interrupted copy is never mistaken
/// for a model, then removes the source.
fn copy_then_remove(src: &Path, dst: &Path) -> Result<()> {
    let part = dst.with_extension("bin.part");
    let copied = std::fs::copy(src, &part)
        .and_then(|_| File::open(&part)?.sync_all())
        .and_then(|_| std::fs::rename(&part, dst));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&part);
        return Err(e).with_context(|| format!("could not copy to {}", dst.display()));
    }
    std::fs::remove_file(src).with_context(|| format!("copied, but could not remove {}", src.display()))
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

    #[test]
    fn env_beats_configured_beats_default() {
        let (env, conf) = (PathBuf::from("/env"), PathBuf::from("/conf"));
        assert_eq!(resolve_models_dir(Some(env.clone()), Some(conf.clone())), env);
        assert_eq!(resolve_models_dir(None, Some(conf.clone())), conf);
        assert_eq!(resolve_models_dir(None, None), default_models_dir());
    }

    /// A fresh, empty folder under the system temp dir.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("meeting-transcriber-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn move_models_moves_only_models_and_keeps_existing_targets() {
        let root = temp_dir("move");
        let (from, to) = (root.join("old"), root.join("new"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("ggml-base.bin"), b"base").unwrap();
        std::fs::write(from.join("ggml-small.bin"), b"small-old").unwrap();
        std::fs::write(from.join("notes.txt"), b"not a model").unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(to.join("ggml-small.bin"), b"small-new").unwrap();

        assert_eq!(downloaded_in(&from).len(), 2);
        let mut calls = Vec::new();
        let moved = move_models(&from, &to, |done, total| calls.push((done, total))).unwrap();

        assert_eq!(moved, 1);
        assert_eq!(calls.last(), Some(&(2, 2)));
        assert_eq!(std::fs::read(to.join("ggml-base.bin")).unwrap(), b"base");
        assert!(!from.join("ggml-base.bin").exists());
        // Already in the new folder: neither copy is touched.
        assert_eq!(std::fs::read(to.join("ggml-small.bin")).unwrap(), b"small-new");
        assert!(from.join("ggml-small.bin").exists());
        assert!(from.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_fallback_leaves_no_part_file() {
        let root = temp_dir("copy");
        let (src, dst) = (root.join("ggml-base.bin"), root.join("sub").join("ggml-base.bin"));
        std::fs::write(&src, b"weights").unwrap();
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();

        copy_then_remove(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"weights");
        assert!(!src.exists());
        assert!(!dst.with_extension("bin.part").exists());

        // A missing source fails cleanly, without leaving a partial file.
        assert!(copy_then_remove(&src, &root.join("other.bin")).is_err());
        assert!(!root.join("other.bin.part").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
