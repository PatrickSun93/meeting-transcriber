//! Whisper on the graphics card when there is one, on Windows and Linux
//! (the Mac always uses Metal).
//!
//! A Windows or Linux build of whisper.cpp uses one kind of graphics card
//! or none: a CUDA build needs NVIDIA's driver (and CUDA's own libraries
//! beside it), a Vulkan build a Vulkan driver (NVIDIA, AMD and Intel all
//! ship one), and either fails to start without it. So the package holds
//! the processor build as `meeting-transcriber`, and may hold GPU builds in
//! `cuda/` and `vulkan/` beside it. The processor build starts first, looks
//! for the drivers, and hands over to the best GPU build that can run
//! (`hand_off`), waiting on it. A GPU build that dies before its model has
//! loaded is noted (`gpu-failed.txt` in the app data folder) and not tried
//! again until it is replaced; the next one, or the processor, is used.
//!
//! The choosing is plain logic, tested on every system; finding the
//! drivers asks each system's loader.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Set for a GPU build started by the processor build: where that one is.
pub const LAUNCHER_ENV: &str = "MEETING_TRANSCRIBER_LAUNCHER";
/// Set with it: the file to create once the model has loaded.
const READY_ENV: &str = "MEETING_TRANSCRIBER_READY_FILE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cuda,
    Vulkan,
}

impl Backend {
    /// Best first.
    pub const ALL: [Backend; 2] = [Backend::Cuda, Backend::Vulkan];

    /// The folder beside the program that holds this build.
    pub fn folder(self) -> &'static str {
        match self {
            Backend::Cuda => "cuda",
            Backend::Vulkan => "vulkan",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Backend::Cuda => "CUDA",
            Backend::Vulkan => "Vulkan",
        }
    }

    fn from_folder(name: &str) -> Option<Self> {
        Backend::ALL.into_iter().find(|b| b.folder() == name)
    }
}

/// What was found on this computer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    /// NVIDIA graphics cards CUDA can use (0 without NVIDIA's driver).
    pub nvidia_gpus: usize,
    /// Whether a Vulkan driver is installed.
    pub vulkan: bool,
    /// The GPU builds beside the program.
    pub builds: Vec<Backend>,
}

impl Found {
    fn can_run(&self, backend: Backend) -> bool {
        self.builds.contains(&backend)
            && match backend {
                Backend::Cuda => self.nvidia_gpus > 0,
                Backend::Vulkan => self.vulkan,
            }
    }
}

/// The GPU builds to try, best first: those beside the program whose
/// driver is here and that have not failed before.
pub fn candidates(found: &Found, failed: &[Backend]) -> Vec<Backend> {
    Backend::ALL
        .into_iter()
        .filter(|b| found.can_run(*b) && !failed.contains(b))
        .collect()
}

/// Why Whisper runs on the processor although GPU builds may be wanted.
pub fn cpu_reason(use_gpu: bool, found: &Found, failed: &[Backend]) -> String {
    if !use_gpu {
        return "Using the graphics card is off in Settings.".into();
    }
    if found.builds.is_empty() {
        return "This copy has no graphics-card build beside it (the cuda or vulkan folder of the Windows \
                package), so Whisper runs on the processor."
            .into();
    }
    let tried: Vec<&str> = failed
        .iter()
        .filter(|b| found.can_run(**b))
        .map(|b| b.label())
        .collect();
    if !tried.is_empty() {
        return format!(
            "The {} build did not start on this computer, so Whisper runs on the processor. \
             `meeting-transcriber log` says why.",
            tried.join(" and ")
        );
    }
    "No graphics card Whisper can use was found (no NVIDIA CUDA or Vulkan driver), so Whisper runs on the \
     processor."
        .into()
}

static CPU_REASON: OnceLock<String> = OnceLock::new();

/// Why this copy runs Whisper on the processor, once `hand_off` has looked.
pub fn why_cpu() -> Option<&'static str> {
    CPU_REASON.get().map(String::as_str)
}

/// The program people start: the processor build, also when a GPU build
/// it started is the one asking (for the startup entry).
pub fn launcher_exe() -> Option<PathBuf> {
    std::env::var_os(LAUNCHER_ENV)
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
}

/// Called once the model has loaded: a GPU build started by the processor
/// build tells it that it works.
pub fn mark_ready() {
    if let Some(path) = std::env::var_os(READY_ENV) {
        let _ = std::fs::write(path, b"ready\n");
    }
}

fn failed_path() -> PathBuf {
    crate::models::app_support_dir().join("gpu-failed.txt")
}

/// Tells one copy of a build from the next: its size and modification time.
fn stamp(exe: &Path) -> Option<String> {
    let meta = std::fs::metadata(exe).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{}-{modified}", meta.len()))
}

/// The builds noted in `text` as failed, as long as they are the same copy
/// (`stamp` of each now).
pub fn parse_failures(text: &str, stamp_now: impl Fn(Backend) -> Option<String>) -> Vec<Backend> {
    text.lines()
        .filter_map(|line| {
            let (name, stamp) = line.split_once(' ')?;
            let backend = Backend::from_folder(name.trim())?;
            (stamp_now(backend).as_deref() == Some(stamp.trim())).then_some(backend)
        })
        .collect()
}

fn build_exe(dir: &Path, backend: Backend) -> PathBuf {
    dir.join(backend.folder())
        .join(format!("meeting-transcriber{}", std::env::consts::EXE_SUFFIX))
}

fn failures(dir: &Path) -> Vec<Backend> {
    let text = std::fs::read_to_string(failed_path()).unwrap_or_default();
    parse_failures(&text, |b| stamp(&build_exe(dir, b)))
}

#[cfg(not(target_os = "macos"))]
fn note_failure(dir: &Path, backend: Backend) {
    let mut text = std::fs::read_to_string(failed_path()).unwrap_or_default();
    if let Some(stamp) = stamp(&build_exe(dir, backend)) {
        text.push_str(&format!("{} {stamp}\n", backend.folder()));
        let _ = std::fs::write(failed_path(), text);
    }
}

/// Forgets the builds that failed, so they are tried again.
pub fn forget_failures() {
    let _ = std::fs::remove_file(failed_path());
}

/// Whether any build is noted as failed.
pub fn has_failures() -> bool {
    failed_path().is_file()
}

/// What `doctor` reports about it.
pub fn report() -> Vec<(&'static str, String)> {
    let gpu = crate::transcriber::GPU;
    let mut facts = vec![(
        "whisper_build",
        gpu.map_or("processor".into(), |g| format!("graphics card ({g})")),
    )];
    if cfg!(target_os = "macos") || gpu.is_some() {
        return facts;
    }
    let Some(dir) = launcher_exe().and_then(|e| e.parent().map(Path::to_path_buf)) else {
        return facts;
    };
    let found = find(&dir);
    let failed = failures(&dir);
    let use_gpu = crate::settings::Settings::load().use_gpu;
    facts.push(("nvidia_gpus", found.nvidia_gpus.to_string()));
    facts.push(("vulkan_driver", if found.vulkan { "found" } else { "not found" }.into()));
    let names = |list: &[Backend]| {
        if list.is_empty() {
            "none".to_string()
        } else {
            list.iter().map(|b| b.label()).collect::<Vec<_>>().join(", ")
        }
    };
    facts.push(("gpu_builds", names(&found.builds)));
    facts.push(("gpu_builds_failed", names(&failed)));
    let chosen = candidates(&found, &failed);
    facts.push((
        "whisper_runs_on",
        match chosen.first() {
            Some(b) if use_gpu => format!("graphics card ({})", b.label()),
            _ => format!("processor: {}", cpu_reason(use_gpu, &found, &failed)),
        },
    ));
    facts
}

#[cfg(not(target_os = "macos"))]
fn find(dir: &Path) -> Found {
    let builds: Vec<Backend> = Backend::ALL
        .into_iter()
        .filter(|b| build_exe(dir, *b).is_file())
        .collect();
    Found {
        nvidia_gpus: if builds.contains(&Backend::Cuda) {
            drivers::nvidia_gpus()
        } else {
            0
        },
        vulkan: builds.contains(&Backend::Vulkan) && drivers::vulkan(),
        builds,
    }
}

#[cfg(target_os = "macos")]
fn find(_dir: &Path) -> Found {
    Found::default()
}

/// The CUDA driver API's functions asked for (`CUresult cuInit(unsigned
/// int)`, `CUresult cuDeviceGetCount(int*)`): the cards it can use.
#[cfg(not(target_os = "macos"))]
type CuInit = unsafe extern "system" fn(u32) -> i32;
#[cfg(not(target_os = "macos"))]
type CuDeviceGetCount = unsafe extern "system" fn(*mut i32) -> i32;

#[cfg(not(target_os = "macos"))]
fn count_cuda_devices(init: CuInit, count: CuDeviceGetCount) -> usize {
    let mut n = 0i32;
    // SAFETY: the driver's functions, called as documented.
    unsafe {
        if init(0) != 0 || count(&mut n) != 0 {
            return 0;
        }
    }
    n.max(0) as usize
}

#[cfg(all(unix, not(target_os = "macos")))]
mod drivers {
    use std::ffi::{CStr, c_void};

    #[link(name = "dl")]
    unsafe extern "C" {
        fn dlopen(name: *const std::ffi::c_char, flags: i32) -> *mut c_void;
        fn dlsym(lib: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
        fn dlclose(lib: *mut c_void) -> i32;
    }
    const RTLD_NOW: i32 = 2;

    fn open(name: &CStr) -> Option<*mut c_void> {
        // SAFETY: a library name; a null result is checked.
        let lib = unsafe { dlopen(name.as_ptr(), RTLD_NOW) };
        (!lib.is_null()).then_some(lib)
    }

    /// NVIDIA cards CUDA can use, asked of NVIDIA's driver (`libcuda.so.1`).
    pub fn nvidia_gpus() -> usize {
        let Some(lib) = open(c"libcuda.so.1") else {
            return 0;
        };
        // SAFETY: symbols of the open library, null-checked, with the
        // signatures CUDA documents; the library is closed after.
        unsafe {
            let init = dlsym(lib, c"cuInit".as_ptr());
            let count = dlsym(lib, c"cuDeviceGetCount".as_ptr());
            let n = if init.is_null() || count.is_null() {
                0
            } else {
                super::count_cuda_devices(
                    std::mem::transmute::<*mut c_void, super::CuInit>(init),
                    std::mem::transmute::<*mut c_void, super::CuDeviceGetCount>(count),
                )
            };
            dlclose(lib);
            n
        }
    }

    /// Whether the Vulkan loader, installed with the graphics driver, is here.
    pub fn vulkan() -> bool {
        match open(c"libvulkan.so.1") {
            Some(lib) => {
                // SAFETY: the library just opened.
                unsafe { dlclose(lib) };
                true
            }
            None => false,
        }
    }
}

#[cfg(windows)]
mod drivers {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{s, w};

    /// NVIDIA cards CUDA can use, asked of NVIDIA's driver (`nvcuda.dll`).
    pub fn nvidia_gpus() -> usize {
        // SAFETY: the driver's own library and functions, with the
        // signatures CUDA documents (`CUresult cuInit(unsigned int)`,
        // `CUresult cuDeviceGetCount(int*)`), freed after.
        unsafe {
            let Ok(lib) = LoadLibraryW(w!("nvcuda.dll")) else {
                return 0;
            };
            let n = match (
                GetProcAddress(lib, s!("cuInit")),
                GetProcAddress(lib, s!("cuDeviceGetCount")),
            ) {
                (Some(init), Some(count)) => {
                    type Proc = unsafe extern "system" fn() -> isize;
                    super::count_cuda_devices(
                        std::mem::transmute::<Proc, super::CuInit>(init),
                        std::mem::transmute::<Proc, super::CuDeviceGetCount>(count),
                    )
                }
                _ => 0,
            };
            let _ = FreeLibrary(lib);
            n
        }
    }

    /// Whether the Vulkan loader, installed with the graphics driver, is here.
    pub fn vulkan() -> bool {
        // SAFETY: loads and frees a system library; nothing is called.
        unsafe {
            match LoadLibraryW(w!("vulkan-1.dll")) {
                Ok(lib) => {
                    let _ = FreeLibrary(lib);
                    true
                }
                Err(_) => false,
            }
        }
    }
}

/// On the processor build: runs the app (no arguments, or `--hidden`) or
/// `transcribe` with the best GPU build that works, if any, waiting for it
/// and returning its exit code. None to go on here, on the processor.
#[cfg(not(target_os = "macos"))]
pub fn hand_off(args: &[String]) -> Option<i32> {
    use std::process::Command;

    let app = matches!(args.first().map(String::as_str), None | Some("--hidden"));
    let transcribe = args.first().is_some_and(|a| a == "transcribe");
    if crate::transcriber::GPU.is_some() || std::env::var_os(LAUNCHER_ENV).is_some() || !(app || transcribe) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    let use_gpu = crate::settings::Settings::load().use_gpu;
    let found = find(&dir);
    let mut failed = failures(&dir);
    if app {
        crate::logfile::start();
    }
    if use_gpu {
        let ready = crate::models::app_support_dir().join("gpu-ready.txt");
        for backend in candidates(&found, &failed) {
            let _ = std::fs::remove_file(&ready);
            eprintln!("gpu: starting the {} build", backend.label());
            let mut command = Command::new(build_exe(&dir, backend));
            command.args(args).env(LAUNCHER_ENV, &exe).env(READY_ENV, &ready);
            // CUDA's libraries sit beside the build. Windows looks there
            // by itself; Linux is told to.
            if cfg!(unix) {
                let mut paths = vec![dir.join(backend.folder())];
                paths.extend(
                    std::env::var_os("LD_LIBRARY_PATH")
                        .iter()
                        .flat_map(std::env::split_paths),
                );
                if let Ok(joined) = std::env::join_paths(paths) {
                    command.env("LD_LIBRARY_PATH", joined);
                }
            }
            let status = command.status();
            match status {
                // A crash before its model loaded: the graphics card's
                // build can't run here.
                Ok(status) if !ready.is_file() && crashed(status.code(), status_signal(&status)) => eprintln!(
                    "gpu: the {} build crashed before its model loaded ({status}); trying the next",
                    backend.label()
                ),
                // Ended as it should, or with an ordinary error (a missing
                // file, say), which the processor build would meet too.
                Ok(status) => {
                    let _ = std::fs::remove_file(&ready);
                    return Some(status.code().unwrap_or(1));
                }
                Err(e) => eprintln!("gpu: could not start the {} build: {e}", backend.label()),
            }
            note_failure(&dir, backend);
            failed.push(backend);
        }
    }
    let reason = cpu_reason(use_gpu, &found, &failed);
    eprintln!("gpu: {reason}");
    let _ = CPU_REASON.set(reason);
    None
}

/// Whether a build's exit means it crashed rather than ended with an
/// error of its own (which exits with 1): a missing library (the loader
/// exits 127 on Linux; Windows reports an NTSTATUS error, a negative code),
/// an abort (code 3 on Windows), or a signal on Linux.
pub fn crashed(code: Option<i32>, signal: Option<i32>) -> bool {
    match code {
        _ if signal.is_some() => true,
        Some(c) if cfg!(windows) => c < 0 || c == 3,
        Some(c) => c == 127 || c == 134,
        None => true,
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn status_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt as _;
    status.signal()
}

#[cfg(windows)]
fn status_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

#[cfg(target_os = "macos")]
pub fn hand_off(_args: &[String]) -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(nvidia: usize, vulkan: bool, builds: &[Backend]) -> Found {
        Found {
            nvidia_gpus: nvidia,
            vulkan,
            builds: builds.to_vec(),
        }
    }

    #[test]
    fn the_best_build_whose_driver_is_here_comes_first() {
        let both = [Backend::Cuda, Backend::Vulkan];
        assert_eq!(candidates(&found(1, true, &both), &[]), both);
        // AMD or Intel: no CUDA.
        assert_eq!(candidates(&found(0, true, &both), &[]), [Backend::Vulkan]);
        // The package without the CUDA build.
        assert_eq!(candidates(&found(1, true, &[Backend::Vulkan]), &[]), [Backend::Vulkan]);
        // CUDA failed before: Vulkan.
        assert_eq!(candidates(&found(1, true, &both), &[Backend::Cuda]), [Backend::Vulkan]);
        assert_eq!(candidates(&found(0, false, &both), &[]), []);
    }

    #[test]
    fn the_processor_is_explained() {
        let both = [Backend::Cuda, Backend::Vulkan];
        assert!(cpu_reason(false, &found(1, true, &both), &[]).contains("off in Settings"));
        assert!(cpu_reason(true, &found(1, true, &[]), &[]).contains("no graphics-card build"));
        assert!(cpu_reason(true, &found(0, false, &both), &[]).contains("No graphics card"));
        let failed = cpu_reason(true, &found(1, true, &both), &[Backend::Cuda, Backend::Vulkan]);
        assert!(failed.starts_with("The CUDA and Vulkan build did not start"));
    }

    #[test]
    fn crashes_are_told_from_errors() {
        assert!(!crashed(Some(0), None));
        assert!(!crashed(Some(1), None));
        assert!(crashed(None, Some(11)));
        assert!(crashed(None, None));
        if cfg!(windows) {
            assert!(crashed(Some(0xC000_0135_u32 as i32), None));
            assert!(crashed(Some(3), None));
        } else {
            assert!(crashed(Some(127), None));
            assert!(!crashed(Some(3), None));
        }
    }

    #[test]
    fn failures_count_for_the_same_copy_only() {
        let text = "cuda 100-5\nvulkan 200-6\nnonsense\nother 1-1\n";
        let now = |b: Backend| match b {
            Backend::Cuda => Some("100-5".to_string()),
            // Replaced since: tried again.
            Backend::Vulkan => Some("300-9".to_string()),
        };
        assert_eq!(parse_failures(text, now), [Backend::Cuda]);
        assert_eq!(parse_failures(text, |_| None), []);
    }
}
