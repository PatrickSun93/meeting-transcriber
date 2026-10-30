//! Windows: Explorer, its folder dialog, and the console.

use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::process::Command;

use super::{Picked, backslashed, spawn};

pub const SHOW_IN_FILES: &str = "Show in Explorer";
pub const THIS_COMPUTER: &str = "this PC";
/// What the Others source records.
pub const SYSTEM_AUDIO: &str = "PC audio";
pub const NO_MIC_HINT: &str = "Check Settings > System > Sound > Input";
/// Shown above the path field that stands in for a folder dialog.
pub const NO_DIALOG_HINT: &str = "The folder dialog could not be opened, so type the folder's path.";
pub const MIC_ACCESS_HINT: &str = "Allow microphone access in Settings > Privacy & security > Microphone, \
     including \"Let desktop apps access your microphone\"";

/// Selects `path` in Explorer.
pub fn reveal(path: &Path) {
    // Explorer wants `/select,"<path>"` as written; Rust's own quoting
    // would wrap the whole argument and Explorer would not parse it.
    // (Windows paths cannot contain quotes.)
    let mut arg = std::ffi::OsString::from("/select,\"");
    arg.push(backslashed(path));
    arg.push("\"");
    spawn(Command::new("explorer").raw_arg(arg));
}

/// Opens a folder in Explorer, creating it first so there is something to show.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    spawn(Command::new("explorer").arg(backslashed(dir)));
}

/// The Explorer folder dialog (`IFileOpenDialog` picking folders). Runs on
/// the picker's own thread, which gets its own COM apartment.
pub(super) fn run_picker(prompt: &str, start: &Path) -> Picked {
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    // SAFETY: COM is set up and torn down on this thread around the dialog,
    // and every COM object is released inside `show_folder_dialog`.
    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_err() {
            return Picked::NoDialog;
        }
        let picked = show_folder_dialog(prompt, start).unwrap_or(Picked::NoDialog);
        CoUninitialize();
        picked
    }
}

/// Shows the dialog and waits for it. An error means it could not be shown.
///
/// # Safety
/// COM must be initialised (single-threaded apartment) on this thread.
unsafe fn show_folder_dialog(prompt: &str, start: &Path) -> windows::core::Result<Picked> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
        SIGDN_FILESYSPATH,
    };
    use windows::core::{HRESULT, HSTRING};

    // SAFETY: plain COM calls on interfaces this function owns; the path
    // string the dialog allocates is freed with CoTaskMemFree as documented.
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
        dialog.SetTitle(&HSTRING::from(prompt))?;
        if start.is_dir()
            && let Ok(folder) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(backslashed(start).as_path()), None)
        {
            // Not fatal: the dialog then opens where it was last used.
            let _ = dialog.SetFolder(&folder);
        }
        if let Err(e) = dialog.Show(None) {
            return if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                Ok(Picked::Cancelled)
            } else {
                Err(e)
            };
        }
        let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = std::ffi::OsString::from_wide(name.as_wide());
        CoTaskMemFree(Some(name.0 as *const _));
        Ok(Picked::Folder(path.into()))
    }
}

/// Connects stdout and stderr to the console the app was started from, if
/// any. Release builds are GUI programs with no console window of their own,
/// so without this `meeting-transcriber transcribe` would print nothing.
pub fn attach_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: takes no pointers, and fails harmlessly (returns 0) when the
    // app was not started from a console.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
