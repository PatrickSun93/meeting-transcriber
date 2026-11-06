//! Screen recording on Windows: the area copied from the screen with GDI
//! 30 times a second, with the pointer drawn in (and scaled down by GDI
//! when the video is smaller than the area), made NV12 (`nv12.rs`), and
//! encoded by Media Foundation's sink writer into the MP4 file: H.264, and
//! AAC for the sound, the PC's sound and the microphone mixed into one
//! track (`mixer.rs`). Each frame is stamped with when it was taken. The
//! windows that show the recording are kept out of it by their display
//! affinity, as for a scrolling screenshot. Media Foundation's libraries
//! are loaded only for a recording: Windows N has them only with the Media
//! Feature Pack, and linked the usual way the app would not start there.

use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use crossbeam_channel::Sender;
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDC, GetDIBits, HALFTONE, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject, SetBrushOrgEx,
    SetStretchBltMode, StretchBlt,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaBuffer, IMFMediaType, IMFSample, IMFSinkWriter, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_VIDEO_NOMINAL_RANGE,
    MF_MT_YUV_MATRIX, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION, MFAudioFormat_AAC, MFAudioFormat_PCM, MFMediaType_Audio,
    MFMediaType_Video, MFNominalRange_16_235, MFSTARTUP_LITE, MFTranscodeContainerType_MPEG4, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFVideoTransferMatrix_BT709, eAVEncH264VProfile_Main,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, HICON, ICONINFO,
};
use windows::core::{GUID, HRESULT, HSTRING, Interface, PCSTR, PCWSTR, s, w};

use super::mixer::Mixer;
use super::{FPS, FrameClock, Recorded, Sound, bitrate, nv12, scaled_down, timer, video_size};
use crate::audio::{self, AudioPacket};
use crate::capture::scrolling::Area;
use crate::transcript::Speaker;

/// The sound's rate, mono, and its AAC bytes a second (128 kbit/s); the
/// encoder takes 44.1 or 48 kHz.
const AUDIO_RATE: u32 = 48_000;
const AAC_BYTES_PER_SECOND: u32 = 16_000;
/// How long the mix waits for a source that has gone quiet.
const AUDIO_LAG: Duration = Duration::from_millis(500);
/// Media Foundation's time unit: 100 nanoseconds.
const UNITS_PER_SECOND: u64 = 10_000_000;
/// How long the screen may go unread at the start before the recording
/// gives up. Later, while Windows shows its lock screen or a UAC prompt,
/// it can't be read for minutes, and the video keeps its last picture.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(5);
const NO_MEDIA_FOUNDATION: &str = "Windows has no Media Foundation (Windows N needs the Media Feature Pack)";

pub fn unsupported() -> Option<&'static str> {
    None
}

/// A recording on its thread.
pub struct Recorder {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Result<Recorded>>,
    failed: Arc<Mutex<Option<String>>>,
}

impl Recorder {
    pub fn start(area: &Area, path: &Path, sound: Sound) -> Result<Recorder> {
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(Mutex::new(None));
        let (ready_tx, ready) = crossbeam_channel::bounded(1);
        let (area, path) = (*area, path.to_path_buf());
        let (thread_stop, thread_failed) = (stop.clone(), failed.clone());
        let thread = std::thread::Builder::new()
            .name("screen recording".into())
            .spawn(move || {
                let result = run(&area, &path, sound, &thread_stop, &ready_tx);
                if let Err(e) = &result {
                    let why = format!("{e:#}");
                    *thread_failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(why.clone());
                    let _ = ready_tx.try_send(Err(anyhow!(why)));
                }
                result
            })
            .context("could not start the recording's thread")?;
        match ready.recv() {
            Ok(Ok(())) => Ok(Recorder { stop, thread, failed }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => bail!("the recording's thread ended"),
        }
    }

    pub fn failure(&self) -> Option<String> {
        self.failed.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn stop(self) -> Result<Recorded> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread
            .join()
            .map_err(|_| anyhow!("the recording's thread crashed"))?
    }
}

/// The recording's thread: Media Foundation on, the recording, and off.
fn run(area: &Area, path: &Path, sound: Sound, stop: &AtomicBool, ready: &Sender<Result<()>>) -> Result<Recorded> {
    let mf = MediaFoundation::load()?;
    // SAFETY: COM and Media Foundation are started and stopped on this
    // thread, in pairs; what the recording made is gone before they stop.
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = (mf.startup)(MF_VERSION, MFSTARTUP_LITE)
            .ok()
            .context(NO_MEDIA_FOUNDATION)
            .and_then(|()| {
                let recorded = record(&mf, area, path, sound, stop, ready);
                let _ = (mf.shutdown)();
                recorded
            });
        if com.is_ok() {
            CoUninitialize();
        }
        result
    }
}

fn record(
    mf: &MediaFoundation,
    area: &Area,
    path: &Path,
    sound: Sound,
    stop: &AtomicBool,
    ready: &Sender<Result<()>>,
) -> Result<Recorded> {
    let [left, top, right, bottom] = area.desktop();
    let size = video_size((right - left) as u32, (bottom - top) as u32);
    let mut screen = Screen::new([left, top], [right - left, bottom - top], size)?;
    // The sound first: whether there is any decides the file's tracks.
    let (tx, packets) = crossbeam_channel::unbounded();
    let mut sources = Vec::new();
    for (wanted, speaker) in [(sound.computer, Speaker::Others), (sound.microphone, Speaker::Me)] {
        if wanted {
            match audio::start_at(speaker, AUDIO_RATE, tx.clone()) {
                Ok(capture) => sources.push(capture),
                Err(e) => eprintln!("screen recording: without {}: {e:#}", speaker.label()),
            }
        }
    }
    let writer = Writer::new(mf, path, size, !sources.is_empty())?;
    let _ = ready.send(Ok(()));

    let started = Instant::now();
    let mut clock = FrameClock::new(FPS);
    let lag = samples(AUDIO_LAG);
    let mut mixer = Mixer::new(2, lag);
    // When the last frame was taken, once one was.
    let mut last: Option<Duration> = None;
    let recorded = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            let at = started.elapsed();
            match screen.grab() {
                Ok(pixels) => {
                    let frame = nv12::from_bgra(pixels, size[0] as usize * 4, size[0] as usize, size[1] as usize);
                    writer.video(&frame, at, clock.interval())?;
                    last = Some(at);
                }
                Err(e) if last.is_none() && at > FIRST_FRAME_WAIT => return Err(e),
                // Skipped: the video holds the frame before.
                Err(_) => {}
            }
            let now = samples(started.elapsed());
            mix(&mut mixer, packets.try_iter(), now);
            writer.audio(mixer.take(now))?;
            std::thread::sleep(clock.wait_after(started.elapsed()));
        }
        Ok(())
    })();
    // However it ended: the sound up to the last frame, then the file is
    // finished, so what was recorded plays.
    drop(sources);
    let length = last.map_or(Duration::ZERO, |at| at + clock.interval());
    let end = samples(length);
    mix(&mut mixer, packets.try_iter(), end);
    let sound = writer.audio(mixer.take(end + lag));
    let finished = writer.finish();
    match recorded {
        Ok(()) => {
            sound?;
            finished?;
            Ok(Recorded {
                seconds: length.as_secs_f64(),
            })
        }
        Err(e) if finished.is_ok() && last.is_some() => Err(e.context(format!(
            "it stopped at {} (the video up to then is saved)",
            timer(length.as_secs())
        ))),
        Err(e) => Err(e),
    }
}

/// How many sound samples `time` is.
fn samples(time: Duration) -> u64 {
    (time.as_secs_f64() * f64::from(AUDIO_RATE)) as u64
}

/// Lays `packets` into the mix: the PC's sound first, then the microphone.
fn mix(mixer: &mut Mixer, packets: impl Iterator<Item = AudioPacket>, now: u64) {
    for packet in packets {
        let source = usize::from(packet.speaker == Speaker::Me);
        mixer.push(source, &packet.samples, now);
    }
}

/// The area of the screen, copied into a bitmap of the video's size.
struct Screen {
    screen: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    /// The area's top-left corner and size on the desktop, in pixels.
    origin: [i32; 2],
    source: [i32; 2],
    size: [u32; 2],
    /// The video is the area scaled down; else its top-left part (all but
    /// an odd last column or row), copied pixel for pixel.
    scaled: bool,
    pixels: Vec<u8>,
}

impl Screen {
    fn new(origin: [i32; 2], source: [i32; 2], size: [u32; 2]) -> Result<Screen> {
        let scaled = scaled_down([source[0] as u32, source[1] as u32], size);
        // SAFETY: each handle is checked here and released in `drop`.
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                bail!("could not read the screen");
            }
            let memory = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, size[0] as i32, size[1] as i32);
            let made = Screen {
                screen,
                memory,
                bitmap,
                origin,
                source,
                size,
                scaled,
                pixels: vec![0; size[0] as usize * size[1] as usize * 4],
            };
            if memory.is_invalid() || bitmap.is_invalid() {
                bail!("could not make a picture of the screen's size");
            }
            // Averages the pixels it drops, rather than skipping them.
            SetStretchBltMode(memory, HALFTONE);
            let _ = SetBrushOrgEx(memory, 0, 0, None);
            Ok(made)
        }
    }

    /// The area as it is now, with the pointer: BGRA, rows from the top.
    fn grab(&mut self) -> Result<&[u8]> {
        let ([x, y], [sw, sh]) = (self.origin, self.source);
        let (w, h) = (self.size[0] as i32, self.size[1] as i32);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Negative: rows from the top.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the handles are alive until `drop`; the bitmap is
        // selected out of the memory DC before GetDIBits reads it, and
        // `pixels` holds `h` rows of `w` 32-bit pixels.
        let lines = unsafe {
            let old: HGDIOBJ = SelectObject(self.memory, self.bitmap.into());
            // Without CAPTUREBLT, which makes the pointer flicker; the
            // pointer is drawn in instead.
            let copied = if self.scaled {
                StretchBlt(self.memory, 0, 0, w, h, Some(self.screen), x, y, sw, sh, SRCCOPY).as_bool()
            } else {
                BitBlt(self.memory, 0, 0, w, h, Some(self.screen), x, y, SRCCOPY).is_ok()
            };
            if copied {
                self.draw_pointer();
            }
            let _ = SelectObject(self.memory, old);
            if !copied {
                bail!("could not copy the screen");
            }
            GetDIBits(
                self.memory,
                self.bitmap,
                0,
                h as u32,
                Some(self.pixels.as_mut_ptr().cast::<c_void>()),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        if lines != h {
            bail!("could not read the copy of the screen");
        }
        Ok(&self.pixels)
    }

    /// Draws the pointer where it is, if it is shown and over the area.
    ///
    /// # Safety
    /// The bitmap is selected into the memory DC.
    unsafe fn draw_pointer(&self) {
        let mut cursor = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: plain queries into structures of the documented sizes;
        // the bitmaps GetIconInfo makes are deleted here.
        unsafe {
            if GetCursorInfo(&mut cursor).is_err() || cursor.flags.0 & CURSOR_SHOWING.0 == 0 {
                return;
            }
            let icon = HICON::from(cursor.hCursor);
            let mut icon_info = ICONINFO::default();
            if GetIconInfo(icon, &mut icon_info).is_err() {
                return;
            }
            for bitmap in [icon_info.hbmMask, icon_info.hbmColor] {
                if !bitmap.is_invalid() {
                    let _ = DeleteObject(bitmap.into());
                }
            }
            let (fx, fy) = if self.scaled {
                (
                    self.size[0] as f32 / self.source[0].max(1) as f32,
                    self.size[1] as f32 / self.source[1].max(1) as f32,
                )
            } else {
                (1.0, 1.0)
            };
            let at = |p: i32, origin: i32, f: f32, hotspot: u32| ((p - origin) as f32 * f) as i32 - hotspot as i32;
            let x = at(cursor.ptScreenPos.x, self.origin[0], fx, icon_info.xHotspot);
            let y = at(cursor.ptScreenPos.y, self.origin[1], fy, icon_info.yHotspot);
            let _ = DrawIconEx(self.memory, x, y, icon, 0, 0, 0, None, DI_NORMAL);
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // SAFETY: the handles were made in `new` and are not used after this.
        unsafe {
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
            if !self.memory.is_invalid() {
                let _ = DeleteDC(self.memory);
            }
            ReleaseDC(None, self.screen);
        }
    }
}

/// A system library, loaded while this lives.
struct Library(HMODULE);

impl Library {
    /// `name` from System32 (and nowhere else).
    fn open(name: PCWSTR) -> Result<Library> {
        // SAFETY: loads a system library, freed in `drop`.
        unsafe { LoadLibraryExW(name, None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
            .map(Library)
            .context(NO_MEDIA_FOUNDATION)
    }

    /// Its function `name`, as `F`.
    ///
    /// # Safety
    /// `F` is a function pointer of the function's documented signature.
    unsafe fn function<F: Copy>(&self, name: PCSTR) -> Result<F> {
        // SAFETY: the library is loaded; the caller vouches for `F`, and
        // the sizes are checked.
        unsafe {
            let found = GetProcAddress(self.0, name).context(NO_MEDIA_FOUNDATION)?;
            assert_eq!(std::mem::size_of::<F>(), std::mem::size_of_val(&found));
            Ok(std::mem::transmute_copy(&found))
        }
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: loaded in `open`; nothing of it is used after this.
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

/// A Media Foundation function that hands back a new object.
type Make = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;

/// Media Foundation's functions this file calls (its objects' methods need
/// no lookup), from `mfplat.dll` and `mfreadwrite.dll`.
struct MediaFoundation {
    startup: unsafe extern "system" fn(u32, u32) -> HRESULT,
    shutdown: unsafe extern "system" fn() -> HRESULT,
    create_attributes: unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT,
    create_media_type: Make,
    create_memory_buffer: unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT,
    create_sample: Make,
    create_sink_writer: unsafe extern "system" fn(PCWSTR, *mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT,
    /// Last, so the functions above are never used after them.
    _libraries: [Library; 2],
}

impl MediaFoundation {
    fn load() -> Result<MediaFoundation> {
        let plat = Library::open(w!("mfplat.dll"))?;
        let readwrite = Library::open(w!("mfreadwrite.dll"))?;
        // SAFETY: each as Microsoft documents it: `HRESULT MFStartup(ULONG,
        // DWORD)`, `HRESULT MFShutdown()`, `HRESULT
        // MFCreateAttributes(IMFAttributes**, UINT32)`, `HRESULT
        // MFCreateMediaType(IMFMediaType**)`, `HRESULT
        // MFCreateMemoryBuffer(DWORD, IMFMediaBuffer**)`, `HRESULT
        // MFCreateSample(IMFSample**)` and `HRESULT
        // MFCreateSinkWriterFromURL(LPCWSTR, IMFByteStream*, IMFAttributes*,
        // IMFSinkWriter**)`.
        unsafe {
            Ok(MediaFoundation {
                startup: plat.function(s!("MFStartup"))?,
                shutdown: plat.function(s!("MFShutdown"))?,
                create_attributes: plat.function(s!("MFCreateAttributes"))?,
                create_media_type: plat.function(s!("MFCreateMediaType"))?,
                create_memory_buffer: plat.function(s!("MFCreateMemoryBuffer"))?,
                create_sample: plat.function(s!("MFCreateSample"))?,
                create_sink_writer: readwrite.function(s!("MFCreateSinkWriterFromURL"))?,
                _libraries: [plat, readwrite],
            })
        }
    }

    fn attributes(&self, size: u32) -> windows::core::Result<IMFAttributes> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: the documented call, into a pointer it fills.
        unsafe { handed_back((self.create_attributes)(&mut raw, size), raw) }
    }

    fn media_type(&self) -> windows::core::Result<IMFMediaType> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: the documented call, into a pointer it fills.
        unsafe { handed_back((self.create_media_type)(&mut raw), raw) }
    }

    fn memory_buffer(&self, length: u32) -> windows::core::Result<IMFMediaBuffer> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: the documented call, into a pointer it fills.
        unsafe { handed_back((self.create_memory_buffer)(length, &mut raw), raw) }
    }

    fn sample(&self) -> windows::core::Result<IMFSample> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: the documented call, into a pointer it fills.
        unsafe { handed_back((self.create_sample)(&mut raw), raw) }
    }

    /// A sink writer making the file at `path`, with no byte stream of
    /// its own.
    fn sink_writer(&self, path: &Path, attributes: &IMFAttributes) -> windows::core::Result<IMFSinkWriter> {
        let path = HSTRING::from(path);
        let mut raw = std::ptr::null_mut();
        // SAFETY: the documented call, with a null-terminated path and
        // attributes that live through it, into a pointer it fills.
        unsafe {
            let result = (self.create_sink_writer)(
                PCWSTR(path.as_ptr()),
                std::ptr::null_mut(),
                attributes.as_raw(),
                &mut raw,
            );
            handed_back(result, raw)
        }
    }
}

/// The object a Media Foundation function handed back, with its result.
///
/// # Safety
/// `raw` is what the function put out as a `T`, with `result`.
unsafe fn handed_back<T: Interface>(result: HRESULT, raw: *mut c_void) -> windows::core::Result<T> {
    result.ok()?;
    if raw.is_null() {
        return Err(windows::core::Error::empty());
    }
    // SAFETY: a reference the function handed over to the caller.
    Ok(unsafe { T::from_raw(raw) })
}

/// Media Foundation's sink writer, making the MP4 file.
struct Writer<'a> {
    mf: &'a MediaFoundation,
    writer: IMFSinkWriter,
    video: u32,
    audio: Option<u32>,
}

impl<'a> Writer<'a> {
    fn new(mf: &'a MediaFoundation, path: &Path, size: [u32; 2], sound: bool) -> Result<Writer<'a>> {
        let made = || -> windows::core::Result<Writer<'a>> {
            // SAFETY: Media Foundation's documented calls, with types it
            // made; the writer keeps what it needs.
            unsafe {
                let attributes = mf.attributes(1)?;
                attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
                let writer = mf.sink_writer(path, &attributes)?;

                let encoded = video_type(mf, &MFVideoFormat_H264, size)?;
                encoded.SetUINT32(&MF_MT_AVG_BITRATE, bitrate(size))?;
                encoded.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;
                let video = writer.AddStream(&encoded)?;
                let frames = video_type(mf, &MFVideoFormat_NV12, size)?;
                frames.SetUINT32(&MF_MT_DEFAULT_STRIDE, size[0])?;
                writer.SetInputMediaType(video, &frames, None)?;

                let audio = if sound {
                    let encoded = audio_type(mf, &MFAudioFormat_AAC)?;
                    encoded.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;
                    let audio = writer.AddStream(&encoded)?;
                    let pcm = audio_type(mf, &MFAudioFormat_PCM)?;
                    pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2)?;
                    pcm.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_RATE * 2)?;
                    writer.SetInputMediaType(audio, &pcm, None)?;
                    Some(audio)
                } else {
                    None
                };
                writer.BeginWriting()?;
                Ok(Writer {
                    mf,
                    writer,
                    video,
                    audio,
                })
            }
        };
        made().map_err(|e| anyhow!("could not start the video file {}: {e}", path.display()))
    }

    /// Adds a frame (NV12) taken `at` from the start, lasting `length`.
    fn video(&self, frame: &[u8], at: Duration, length: Duration) -> Result<()> {
        self.write(self.video, frame, units(at), units(length))
            .context("could not add to the video file")
    }

    /// Adds the mixed sound that starts at sample `at`.
    fn audio(&self, (at, samples): (u64, Vec<f32>)) -> Result<()> {
        let Some(stream) = self.audio else {
            return Ok(());
        };
        if samples.is_empty() {
            return Ok(());
        }
        let pcm: Vec<u8> = samples
            .iter()
            .flat_map(|s| ((s * 32767.0).round() as i16).to_le_bytes())
            .collect();
        let start = at * UNITS_PER_SECOND / u64::from(AUDIO_RATE);
        let length = samples.len() as u64 * UNITS_PER_SECOND / u64::from(AUDIO_RATE);
        self.write(stream, &pcm, start as i64, length as i64)
            .context("could not add the sound to the video file")
    }

    fn write(&self, stream: u32, bytes: &[u8], at: i64, length: i64) -> windows::core::Result<()> {
        // SAFETY: the buffer is locked while it is filled with no more
        // than its length, and unlocked before it is handed on.
        unsafe {
            let buffer = self.mf.memory_buffer(bytes.len() as u32)?;
            let mut data: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes.len() as u32)?;
            let sample = self.mf.sample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(at)?;
            sample.SetSampleDuration(length)?;
            self.writer.WriteSample(stream, &sample)
        }
    }

    /// Finishes the file.
    fn finish(self) -> Result<()> {
        // SAFETY: the writer's documented last call.
        unsafe { self.writer.Finalize() }.context("could not finish the video file")
    }
}

/// `time` in Media Foundation's units.
fn units(time: Duration) -> i64 {
    (time.as_nanos() / 100) as i64
}

/// Two numbers in one attribute, as frame sizes and rates are kept.
fn pair(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// A video type of `subtype`, `size`, 30 frames a second, BT.709 colours.
///
/// # Safety
/// Media Foundation is started.
unsafe fn video_type(mf: &MediaFoundation, subtype: &GUID, size: [u32; 2]) -> windows::core::Result<IMFMediaType> {
    // SAFETY: setting attributes of a type made here.
    unsafe {
        let kind = mf.media_type()?;
        kind.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        kind.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        kind.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        kind.SetUINT64(&MF_MT_FRAME_SIZE, pair(size[0], size[1]))?;
        kind.SetUINT64(&MF_MT_FRAME_RATE, pair(FPS, 1))?;
        kind.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1))?;
        kind.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        kind.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        Ok(kind)
    }
}

/// A sound type of `subtype`: 16-bit mono at 48 kHz.
///
/// # Safety
/// Media Foundation is started.
unsafe fn audio_type(mf: &MediaFoundation, subtype: &GUID) -> windows::core::Result<IMFMediaType> {
    // SAFETY: setting attributes of a type made here.
    unsafe {
        let kind = mf.media_type()?;
        kind.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        kind.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        kind.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        kind.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
        kind.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
        Ok(kind)
    }
}
