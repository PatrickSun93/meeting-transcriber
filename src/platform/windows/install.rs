//! Installing on Windows. The download is a zip with no installer, so the
//! program was easy to lose: run from wherever it was unzipped (or from
//! inside the zip, in a temporary folder Windows empties), with nothing in
//! the Start menu. Installing copies it, and the graphics-card builds
//! beside it, to `%LOCALAPPDATA%\Programs\Meeting Transcriber`, and puts a
//! shortcut in the Start menu and, if wanted, on the desktop. No
//! administrator rights are needed.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface as _};

const EXE: &str = "meeting-transcriber.exe";
const SHORTCUT: &str = "Meeting Transcriber.lnk";

/// Where the program is installed.
pub fn install_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Programs")
        .join("Meeting Transcriber")
}

fn start_menu_shortcut() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join(SHORTCUT),
    )
}

fn desktop_shortcut() -> Option<PathBuf> {
    Some(dirs::desktop_dir()?.join(SHORTCUT))
}

/// The folder the program people start is in.
fn running_dir() -> Option<PathBuf> {
    crate::gpu::launcher_exe()?.parent().map(Path::to_path_buf)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// Whether this copy is the installed one (with its Start menu shortcut).
pub fn installed() -> bool {
    running_dir().is_some_and(|dir| same_dir(&dir, &install_dir()))
        && start_menu_shortcut().is_some_and(|p| p.is_file())
}

/// Whether this copy runs from the temporary folder: opened from inside
/// the zip, so it is gone once Explorer cleans up.
pub fn running_from_temp() -> bool {
    running_dir().is_some_and(|dir| {
        let temp = std::env::temp_dir();
        let norm = |p: &Path| p.to_string_lossy().to_lowercase();
        norm(&dir).starts_with(&norm(&temp))
    })
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Copies this copy into `install_dir` (unless it already runs from
/// there) and makes the shortcuts. Returns the installed program.
pub fn install(desktop: bool) -> Result<PathBuf> {
    let from = running_dir().context("could not find where the app is")?;
    let to = install_dir();
    let exe = to.join(EXE);
    if !same_dir(&from, &to) {
        std::fs::create_dir_all(&to).with_context(|| format!("could not make {}", to.display()))?;
        std::fs::copy(from.join(EXE), &exe).map_err(|e| {
            anyhow!(
                "could not copy the program to {} ({e}). If Meeting Transcriber is open from there, quit it \
                 (also in the notification area), then install again.",
                to.display()
            )
        })?;
        // The builds this copy has replace the installed ones; others stay
        // (a CUDA add-on the app downloaded: it offers an update if it no
        // longer fits).
        for gpu in crate::gpu::Backend::ALL {
            let folder = gpu.folder();
            if from.join(folder).is_dir() {
                let _ = std::fs::remove_dir_all(to.join(folder));
                copy_dir(&from.join(folder), &to.join(folder))
                    .with_context(|| format!("could not copy the {folder} folder"))?;
            }
        }
    }
    let start = start_menu_shortcut().context("no Start menu folder")?;
    make_shortcut(&start, &exe)?;
    if desktop && let Some(path) = desktop_shortcut() {
        make_shortcut(&path, &exe)?;
    }
    // Opening at login starts the installed copy from now on.
    if super::system::open_at_login() {
        super::system::set_open_at_login_for(&exe)?;
    }
    Ok(exe)
}

/// Starts the installed copy (after `install`).
pub fn open_installed(exe: &Path) -> Result<()> {
    std::process::Command::new(exe)
        .spawn()
        .map(drop)
        .with_context(|| format!("could not open {}", exe.display()))
}

/// Takes the shortcuts and the login entry away. The program's folder is
/// left for the user to delete (a running program can't delete itself).
pub fn uninstall() -> Result<PathBuf> {
    for path in [start_menu_shortcut(), desktop_shortcut()].into_iter().flatten() {
        let _ = std::fs::remove_file(path);
    }
    super::system::set_open_at_login(false)?;
    Ok(install_dir())
}

/// Writes a `.lnk` at `path` that opens `exe`, on a thread of its own with
/// its own COM apartment.
fn make_shortcut(path: &Path, exe: &Path) -> Result<()> {
    let shown = path.display().to_string();
    let (path, exe) = (path.to_path_buf(), exe.to_path_buf());
    std::thread::spawn(move || {
        // SAFETY: COM is set up and torn down on this thread around the
        // calls; the interfaces are released when dropped, before that.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            let result = (|| -> windows::core::Result<()> {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.SetPath(&HSTRING::from(exe.as_os_str()))?;
                if let Some(dir) = exe.parent() {
                    link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
                }
                link.SetDescription(&HSTRING::from("Live meeting transcription and dictation"))?;
                link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
                let file: IPersistFile = link.cast()?;
                file.Save(&HSTRING::from(path.as_os_str()), true)
            })();
            CoUninitialize();
            result
        }
    })
    .join()
    .map_err(|_| anyhow!("the shortcut thread failed"))?
    .with_context(|| format!("could not make the shortcut {shown}"))
}
