//! CUDA support, downloaded when it is needed. The CUDA build of Whisper
//! and CUDA's runtime libraries come to about 600 MB, which only PCs with
//! an NVIDIA card use, so the Windows and Linux packages leave them out.
//! On such a PC the app offers this add-on: a zip of `cuda/` (the build,
//! the libraries and `version.txt`), published with each release, which is
//! unpacked beside the program, where `gpu` finds it at the next start.
//!
//! An add-on works only with the program of its own version (it is a whole
//! build of the app), so it is downloaded from the release of this version,
//! and one left from another version is offered again as an update.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::gpu::{self, Backend};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES: &str = "https://github.com/PatrickSun93/meeting-transcriber/releases/download";
/// Overrides where the add-on is downloaded from (a URL or a local zip).
pub const SOURCE_ENV: &str = "MEETING_TRANSCRIBER_CUDA_ADDON";
/// Roughly, for the button.
pub const SIZE_MB: u32 = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// No NVIDIA card here, or no add-on for this system (the Mac, ARM).
    NotNeeded,
    /// An NVIDIA card, and no add-on yet.
    Missing,
    /// An add-on for another version of the program (that version).
    Outdated(String),
    Installed,
}

impl State {
    /// Whether the app should offer to download it.
    pub fn offer(&self) -> bool {
        matches!(self, State::Missing | State::Outdated(_))
    }

    pub fn describe(&self) -> String {
        match self {
            State::NotNeeded => "not needed (no NVIDIA card, or none for this system)".into(),
            State::Missing => "not installed".into(),
            State::Outdated(v) => format!("installed for version {v}; this is {VERSION}"),
            State::Installed => "installed".into(),
        }
    }
}

/// The add-on's file in a release, on the systems that have one.
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("Meeting-Transcriber-Windows-x86_64-CUDA.zip")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("Meeting-Transcriber-Linux-x86_64-CUDA.zip")
    } else {
        None
    }
}

/// Where this version's add-on is downloaded from.
pub fn source() -> Option<String> {
    std::env::var(SOURCE_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| asset_name().map(|a| format!("{RELEASES}/v{VERSION}/{a}")))
}

/// The add-on's state for the program in `dir`, with `nvidia` cards.
pub fn state_in(dir: &Path, nvidia: usize) -> State {
    if asset_name().is_none() || nvidia == 0 {
        return State::NotNeeded;
    }
    if !gpu::build_exe(dir, Backend::Cuda).is_file() {
        return State::Missing;
    }
    match gpu::build_version(dir, Backend::Cuda) {
        Some(v) if v != VERSION => State::Outdated(v),
        _ => State::Installed,
    }
}

pub fn state() -> State {
    match gpu::program_dir() {
        Some(dir) => state_in(&dir, gpu::nvidia_gpus()),
        None => State::NotNeeded,
    }
}

/// Downloads (or, for a local zip, reads) the add-on and unpacks it beside
/// the program, reporting the download's (bytes done, bytes total). `from`
/// overrides `source()`.
pub fn install(from: Option<&str>, cancel: Arc<AtomicBool>, progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    let dir = gpu::program_dir().context("could not find where the app is")?;
    let from = match from {
        Some(f) => f.to_string(),
        None => source().context("there is no CUDA support for this system")?,
    };
    if from.starts_with("http://") || from.starts_with("https://") {
        let part = dir.join("cuda-addon.zip.part");
        crate::models::download_file(&from, &part, cancel, progress).map_err(|e| {
            if format!("{e:#}").contains("404") {
                anyhow!(
                    "CUDA support for version {VERSION} has not been published yet (no release v{VERSION} has \
                     it). Until it is, Whisper uses Vulkan."
                )
            } else {
                e
            }
        })?;
        let result = unpack(&part, &dir);
        let _ = std::fs::remove_file(&part);
        result?;
    } else {
        unpack(Path::new(&from), &dir)?;
    }
    Ok(dir.join(Backend::Cuda.folder()))
}

/// Unpacks an add-on zip (holding `cuda/`) into `dir/cuda`, replacing the
/// one there. Checks that it is for this version first.
pub fn unpack(zip_path: &Path, dir: &Path) -> Result<()> {
    let file = File::open(zip_path).with_context(|| format!("could not open {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("the CUDA add-on is not a valid zip")?;
    let folder = Backend::Cuda.folder();
    let fresh = dir.join(format!("{folder}.new"));
    let _ = std::fs::remove_dir_all(&fresh);
    std::fs::create_dir_all(&fresh).with_context(|| format!("could not make {}", fresh.display()))?;
    let unpacked = (|| -> Result<()> {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            // Only what is inside cuda/, and nothing outside it.
            let Some(name) = entry.enclosed_name() else { continue };
            let Ok(rest) = name.strip_prefix(folder) else { continue };
            let target = fresh.join(rest);
            if entry.is_dir() {
                std::fs::create_dir_all(&target)?;
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&target).with_context(|| format!("could not write {}", target.display()))?;
            std::io::copy(&mut entry, &mut out).context("could not unpack the CUDA add-on")?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o777))?;
            }
        }
        let exe = gpu::build_exe(dir, Backend::Cuda);
        let exe_name = exe.file_name().context("no program name")?;
        if !fresh.join(exe_name).is_file() {
            bail!(
                "the zip is not a CUDA add-on (no {folder}/{})",
                exe_name.to_string_lossy()
            );
        }
        let version = std::fs::read_to_string(fresh.join("version.txt")).unwrap_or_default();
        if version.trim() != VERSION {
            bail!(
                "this CUDA add-on is for version {}, and this is {VERSION}",
                if version.trim().is_empty() {
                    "unknown"
                } else {
                    version.trim()
                }
            );
        }
        Ok(())
    })();
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(e);
    }
    let old = dir.join(folder);
    if old.exists() {
        std::fs::remove_dir_all(&old).map_err(|e| {
            let _ = std::fs::remove_dir_all(&fresh);
            anyhow!(
                "could not replace {} ({e}). Quit Meeting Transcriber everywhere (also in the notification area) \
                 and try again.",
                old.display()
            )
        })?;
    }
    std::fs::rename(&fresh, &old).with_context(|| format!("could not put {} in place", old.display()))?;
    // A build that failed before is tried again: this is a new one.
    gpu::forget_failures();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn addon_zip(path: &Path, version: &str, with_exe: bool) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        let exe = format!("cuda/meeting-transcriber{}", std::env::consts::EXE_SUFFIX);
        if with_exe {
            zip.start_file(exe, opts).unwrap();
            zip.write_all(b"build").unwrap();
        }
        zip.start_file("cuda/libcudart.so.12", opts).unwrap();
        zip.write_all(b"lib").unwrap();
        zip.start_file("cuda/version.txt", opts).unwrap();
        zip.write_all(version.as_bytes()).unwrap();
        // Outside cuda/: ignored, and nothing escapes the folder.
        zip.start_file("../escape.txt", opts).unwrap();
        zip.write_all(b"no").unwrap();
        zip.finish().unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mt-cuda-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn addons_unpack_for_this_version_only() {
        let dir = temp_dir("unpack");
        let zip = dir.join("addon.zip");
        addon_zip(&zip, VERSION, true);
        unpack(&zip, &dir).unwrap();
        assert!(gpu::build_exe(&dir, Backend::Cuda).is_file());
        assert!(dir.join("cuda/libcudart.so.12").is_file());
        assert!(!dir.join("escape.txt").exists() && !dir.parent().unwrap().join("escape.txt").exists());
        if asset_name().is_some() {
            assert_eq!(state_in(&dir, 1), State::Installed);
            assert_eq!(state_in(&dir, 0), State::NotNeeded);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(gpu::build_exe(&dir, Backend::Cuda))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }

        // Another version's add-on is refused, and the installed one stays.
        let other = dir.join("other.zip");
        addon_zip(&other, "0.0.1", true);
        let err = unpack(&other, &dir).unwrap_err();
        assert!(format!("{err:#}").contains("for version 0.0.1"));
        assert!(gpu::build_exe(&dir, Backend::Cuda).is_file());
        assert!(!dir.join("cuda.new").exists());

        // A zip without the build is not an add-on.
        let empty = dir.join("empty.zip");
        addon_zip(&empty, VERSION, false);
        assert!(format!("{:#}", unpack(&empty, &dir).unwrap_err()).contains("not a CUDA add-on"));

        // An add-on left from another version is offered as an update.
        std::fs::write(dir.join("cuda/version.txt"), "0.0.1").unwrap();
        if asset_name().is_some() {
            assert_eq!(state_in(&dir, 1), State::Outdated("0.0.1".into()));
            assert!(state_in(&dir, 1).offer());
        }
        assert!(!gpu::build_fits(&dir, Backend::Cuda));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_addons_are_offered_where_there_is_one() {
        let dir = temp_dir("missing");
        let expected = if asset_name().is_some() {
            State::Missing
        } else {
            State::NotNeeded
        };
        assert_eq!(state_in(&dir, 1), expected);
        assert_eq!(state_in(&dir, 0), State::NotNeeded);
        if let Some(asset) = asset_name() {
            assert!(source().unwrap().ends_with(&format!("/v{VERSION}/{asset}")));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
