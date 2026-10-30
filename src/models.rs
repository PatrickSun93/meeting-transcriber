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

/// The old folder of a models move that has not finished. Models still
/// there are found as if they were already in the models folder, so the
/// app keeps working while (or after) a move is interrupted.
static MOVE_SOURCE: RwLock<Option<PathBuf>> = RwLock::new(None);

impl ModelInfo {
    pub fn by_id(id: &str) -> Option<ModelInfo> {
        MODELS.iter().copied().find(|m| m.id == id)
    }

    /// Where the model is: the models folder, or the old folder of an
    /// unfinished move if it is still there. The models folder when it is
    /// in neither.
    pub fn path(&self) -> PathBuf {
        locate(self.file, &models_dir(), move_source().as_deref())
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

/// Settings and, by default, the models: `~/Library/Application Support/
/// Meeting Transcriber` on macOS, `%APPDATA%\Meeting Transcriber` on Windows,
/// `~/.local/share/Meeting Transcriber` on Linux.
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

/// Sets (or clears) the old folder of an unfinished move.
pub fn set_move_source(dir: Option<PathBuf>) {
    *MOVE_SOURCE.write().unwrap_or_else(|e| e.into_inner()) = dir;
}

fn move_source() -> Option<PathBuf> {
    MOVE_SOURCE.read().unwrap_or_else(|e| e.into_inner()).clone()
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
/// files total). A different model file already in `to` is kept and so is
/// the one in `from`; an identical one is the copy an interrupted move
/// finished, so the leftover in `from` is removed. Returns how many were
/// moved. Running it again after an interruption finishes the move.
pub fn move_models(from: &Path, to: &Path, mut progress: impl FnMut(usize, usize)) -> Result<usize> {
    let models = downloaded_in(from);
    std::fs::create_dir_all(to).with_context(|| format!("could not create {}", to.display()))?;
    // The same folder by another name: "moving" would delete the models.
    if same_dir(from, to) {
        return Ok(0);
    }
    let mut moved = 0;
    for (i, (model, size)) in models.iter().enumerate() {
        progress(i, models.len());
        let (src, dst) = (from.join(model.file), to.join(model.file));
        if dst.exists() {
            if file_size(&dst) == Some(*size) && same_contents(&src, &dst) {
                std::fs::remove_file(&src).with_context(|| format!("could not remove the moved {}", src.display()))?;
            }
            continue;
        }
        move_file(&src, &dst).with_context(|| format!("could not move {}", model.file))?;
        moved += 1;
    }
    progress(models.len(), models.len());
    Ok(moved)
}

/// `file` in `dir`, else in `old` (an unfinished move's source) if it is
/// there, else where it would be in `dir`.
fn locate(file: &str, dir: &Path, old: Option<&Path>) -> PathBuf {
    let path = dir.join(file);
    match old.map(|o| o.join(file)) {
        Some(old) if !path.is_file() && old.is_file() => old,
        _ => path,
    }
}

/// Byte-for-byte equal. Reads both files, so only for the rare case of a
/// model in both folders, on the move's own thread.
fn same_contents(a: &Path, b: &Path) -> bool {
    let (Ok(mut a), Ok(mut b)) = (File::open(a), File::open(b)) else {
        return false;
    };
    let (mut buf_a, mut buf_b) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let Ok(n) = read_full(&mut a, &mut buf_a) else {
            return false;
        };
        let Ok(m) = read_full(&mut b, &mut buf_b) else {
            return false;
        };
        if n != m || buf_a[..n] != buf_b[..m] {
            return false;
        }
        if n == 0 {
            return true;
        }
    }
}

/// Fills `buf` unless the file ends first; returns the bytes read.
fn read_full(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Deletes `ggml-*.bin.part` files in `dir`: what an interrupted download
/// or cross-disk move leaves behind. Only call it while neither is running.
/// Returns how many were removed.
pub fn remove_partial_files(dir: &Path) -> usize {
    let partial = |name: &str| name.starts_with("ggml-") && name.ends_with(".bin.part");
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| partial(&e.file_name().to_string_lossy()))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
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
    // Always into the models folder, never the old folder of a move.
    let target = models_dir().join(model.file);
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
        // A different file already in the new folder: neither copy is touched.
        assert_eq!(std::fs::read(to.join("ggml-small.bin")).unwrap(), b"small-new");
        assert!(from.join("ggml-small.bin").exists());
        assert!(from.join("notes.txt").exists());
        // Moving a folder onto itself moves (and deletes) nothing.
        assert_eq!(move_models(&to, &to.join("..").join("new"), |_, _| {}).unwrap(), 0);
        assert_eq!(downloaded_in(&to).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn interrupted_move_is_finished_without_leftovers() {
        let root = temp_dir("resume");
        let (from, to) = (root.join("old"), root.join("new"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        // Stopped mid-way: base was copied but not yet removed from the old
        // folder, small was being copied, medium was not reached.
        std::fs::write(from.join("ggml-base.bin"), b"base").unwrap();
        std::fs::write(to.join("ggml-base.bin"), b"base").unwrap();
        std::fs::write(from.join("ggml-small.bin"), b"small").unwrap();
        std::fs::write(to.join("ggml-small.bin.part"), b"sm").unwrap();
        std::fs::write(from.join("ggml-medium-q5_0.bin"), b"medium").unwrap();
        std::fs::write(to.join("notes.part"), b"not ours").unwrap();

        assert_eq!(remove_partial_files(&to), 1);
        assert!(to.join("notes.part").exists());
        let moved = move_models(&from, &to, |_, _| {}).unwrap();

        assert_eq!(moved, 2);
        assert!(downloaded_in(&from).is_empty());
        assert_eq!(downloaded_in(&to).len(), 3);
        assert_eq!(std::fs::read(to.join("ggml-small.bin")).unwrap(), b"small");
        assert!(!to.join("ggml-small.bin.part").exists());
        assert_eq!(remove_partial_files(&root.join("missing")), 0);
        // While a move is unfinished, models are found in either folder.
        std::fs::write(from.join("ggml-large-v3-q5_0.bin"), b"large").unwrap();
        let large = "ggml-large-v3-q5_0.bin";
        assert_eq!(locate(large, &to, Some(&from)), from.join(large));
        assert_eq!(locate("ggml-small.bin", &to, Some(&from)), to.join("ggml-small.bin"));
        assert_eq!(locate("ggml-base.bin", &from, Some(&to)), to.join("ggml-base.bin"));
        assert_eq!(locate("ggml-tiny.bin", &to, Some(&from)), to.join("ggml-tiny.bin"));
        assert_eq!(locate(large, &to, None), to.join(large));
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
