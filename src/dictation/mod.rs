//! Hold-to-talk dictation, as voice2cursor did it: hold the hotkey and speak,
//! let go and the text is pasted where the cursor is.
//!
//! It runs on two threads of its own, so it works while the window is
//! hidden: the recorder starts and stops the microphone on the hotkey, and
//! the finisher sends each clip to the meeting transcriber's engine (the
//! same Whisper model, loaded once), then copies, pastes and archives the
//! text. Clips are finished one at a time, in order, so two quick
//! dictations never paste over each other.

pub mod archive;
pub mod hotkey;
pub mod ollama;
pub mod voice2cursor;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use chrono::NaiveDateTime;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::audio::dsp::SAMPLE_RATE;
use crate::audio::{AudioPacket, Capture, start_mic};
use crate::platform::{self, Sound};
use crate::transcriber::{DictationJob, Job, Output};
use ollama::OllamaConfig;

/// How many dictations the menu bar item lists.
pub const HISTORY: usize = 10;
/// Me stays muted in a meeting this long after the hotkey comes up, for
/// the end of what was said to clear the meeting's microphone buffers.
const UNMUTE_AFTER: Duration = Duration::from_millis(300);
/// Longest a clip waits for the model to load.
const ENGINE_WAIT: Duration = Duration::from_secs(180);
/// Longest a clip waits for its text (behind other dictations, or a long
/// meeting chunk the engine is in the middle of).
const RESULT_WAIT: Duration = Duration::from_secs(600);
/// How often a recording asks the keyboard whether the hotkey is still
/// held, in case its release was never seen.
const HELD_CHECK: Duration = Duration::from_millis(250);

/// Says whether the hotkey is down right now, or None if it can't tell.
type KeyProbe = Arc<dyn Fn() -> Option<bool> + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub struct DictationConfig {
    /// Off: the hotkey does nothing.
    pub enabled: bool,
    /// None to detect the language.
    pub language: Option<String>,
    pub initial_prompt: Option<String>,
    pub auto_paste: bool,
    pub sounds: bool,
    pub save_recordings: bool,
    pub recordings_dir: PathBuf,
    /// Shorter clips are taken as an accidental press.
    pub min_seconds: f32,
    /// Recording stops on its own after this long.
    pub max_seconds: f32,
    /// Some when the Ollama clean-up is on.
    pub ollama: Option<OllamaConfig>,
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            language: None,
            initial_prompt: None,
            auto_paste: true,
            sounds: true,
            save_recordings: true,
            recordings_dir: archive::default_dir(),
            min_seconds: 0.35,
            max_seconds: 300.0,
            ollama: None,
        }
    }
}

/// The transcription engine, as far as dictation is concerned.
#[derive(Debug, Clone)]
pub enum Engine {
    /// None is loaded; dictation asks the window to load one.
    Missing,
    Loading,
    Ready(Sender<Job>),
    /// Can't be loaded, and why (no model downloaded, say).
    Unavailable(String),
}

/// What the icon shows, most important first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Loading,
    Paused,
    Recording,
    Transcribing,
    Error,
    Ready,
}

/// Dictation's state, for the window and the menu bar item.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub enabled: bool,
    pub paused: bool,
    pub recording: bool,
    /// Clips waiting for their text or being finished.
    pub busy: usize,
    pub engine_ready: bool,
    /// Why the engine is unavailable, if it is.
    pub engine_problem: Option<String>,
    /// The last failure, until the next dictation succeeds.
    pub last_error: Option<(NaiveDateTime, String)>,
    /// The latest dictations, newest first.
    pub history: VecDeque<(NaiveDateTime, String)>,
    /// Goes up with every finished clip, saved or not.
    pub finished: u64,
}

impl Status {
    pub fn phase(&self) -> Phase {
        if self.paused || !self.enabled {
            Phase::Paused
        } else if self.recording {
            Phase::Recording
        } else if self.busy > 0 {
            Phase::Transcribing
        } else if self.engine_problem.is_some() || self.last_error.is_some() {
            Phase::Error
        } else if !self.engine_ready {
            Phase::Loading
        } else {
            Phase::Ready
        }
    }
}

/// A finished recording on its way to text.
struct Clip {
    samples: Vec<f32>,
    /// Where it was archived, if it was.
    wav: Option<PathBuf>,
}

struct Shared {
    config: Mutex<DictationConfig>,
    status: Mutex<Status>,
    engine: Mutex<Engine>,
    engine_changed: Condvar,
    /// Set when a clip needs an engine and none is loaded.
    wants_engine: AtomicBool,
    mute_me: Arc<AtomicBool>,
    key_probe: Mutex<Option<KeyProbe>>,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn status(&self) -> MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn config(&self) -> DictationConfig {
        self.config.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn update(&self, change: impl FnOnce(&mut Status)) {
        change(&mut self.status());
        (self.wake)();
    }

    fn fail(&self, message: String, sounds: bool) {
        eprintln!("dictation: {message}");
        if sounds {
            platform::play_sound(Sound::Error);
        }
        self.update(|s| s.last_error = Some((now(), message)));
    }
}

fn now() -> NaiveDateTime {
    chrono::Local::now().naive_local()
}

/// The running dictation threads.
pub struct Dictation {
    shared: Arc<Shared>,
    keys: Sender<bool>,
}

impl Dictation {
    /// Starts the threads. `wake` is called (from them) whenever the status
    /// changes or an engine is wanted.
    pub fn start(config: DictationConfig, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let enabled = config.enabled;
        let shared = Arc::new(Shared {
            config: Mutex::new(config),
            status: Mutex::new(Status {
                enabled,
                ..Status::default()
            }),
            engine: Mutex::new(Engine::Missing),
            engine_changed: Condvar::new(),
            wants_engine: AtomicBool::new(false),
            mute_me: Arc::default(),
            key_probe: Mutex::default(),
            wake: Box::new(wake),
        });
        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (clip_tx, clip_rx) = crossbeam_channel::unbounded();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("dictation-recorder".into())
            .spawn(move || record_loop(&s, &key_rx, &clip_tx))
            .expect("failed to start the dictation thread");
        let s = shared.clone();
        std::thread::Builder::new()
            .name("dictation-finisher".into())
            .spawn(move || {
                for clip in clip_rx {
                    finish(&s, clip);
                }
            })
            .expect("failed to start the dictation thread");
        Self { shared, keys: key_tx }
    }

    /// The hotkey went down (true) or up. Safe to call from any thread.
    pub fn key_sender(&self) -> impl Fn(bool) + Send + 'static {
        let keys = self.keys.clone();
        move |down| {
            let _ = keys.send(down);
        }
    }

    pub fn set_config(&self, config: DictationConfig) {
        let enabled = config.enabled;
        *self.shared.config.lock().unwrap_or_else(|e| e.into_inner()) = config;
        self.shared.update(|s| s.enabled = enabled);
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.update(|s| s.paused = paused);
    }

    pub fn status(&self) -> Status {
        self.shared.status().clone()
    }

    /// Dismisses the last error.
    pub fn clear_error(&self) {
        self.shared.update(|s| s.last_error = None);
    }

    /// Tells dictation which engine to use; the window calls this whenever
    /// its engine changes.
    pub fn set_engine(&self, engine: Engine) {
        let (ready, problem) = match &engine {
            Engine::Ready(_) => (true, None),
            Engine::Unavailable(why) => (false, Some(why.clone())),
            Engine::Missing | Engine::Loading => (false, None),
        };
        *self.shared.engine.lock().unwrap_or_else(|e| e.into_inner()) = engine;
        self.shared.engine_changed.notify_all();
        let mut status = self.shared.status();
        if status.engine_ready != ready || status.engine_problem != problem {
            status.engine_ready = ready;
            status.engine_problem = problem;
            drop(status);
            (self.shared.wake)();
        }
    }

    /// True once after a clip asked for an engine and none was loaded.
    pub fn take_engine_request(&self) -> bool {
        self.shared.wants_engine.swap(false, Ordering::Relaxed)
    }

    /// Set while the hotkey is held; a meeting session hears Me as silence.
    pub fn mute_me(&self) -> Arc<AtomicBool> {
        self.shared.mute_me.clone()
    }

    /// How a recording can ask whether the hotkey is still down: if its
    /// release is missed (the event tap was turned off for a moment, the
    /// hotkey changed mid-press), the recording still ends.
    pub fn set_key_probe(&self, probe: impl Fn() -> Option<bool> + Send + Sync + 'static) {
        *self.shared.key_probe.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(probe));
    }
}

/// Whether a recording's hotkey was let go of without an event saying so.
/// Only once the key has been seen down: a probe that always says "up"
/// (no permission to ask) must not cut every recording short.
fn missed_release(seen_down: &mut bool, held: Option<bool>) -> bool {
    match held {
        Some(true) => {
            *seen_down = true;
            false
        }
        Some(false) => *seen_down,
        None => false,
    }
}

/// A recording in progress.
struct Recording {
    _capture: Capture,
    audio: Receiver<AudioPacket>,
    samples: Vec<f32>,
    /// Ticks for asking whether the hotkey is still held.
    held_check: Receiver<Instant>,
    seen_down: bool,
}

/// The recorder: starts the microphone when the hotkey goes down and hands
/// the clip to the finisher when it comes up (or the clip gets too long).
fn record_loop(shared: &Shared, keys: &Receiver<bool>, clips: &Sender<Clip>) {
    let mut recording: Option<Recording> = None;
    let mut unmute_at: Option<Instant> = None;
    loop {
        let audio = recording.as_ref().map(|r| r.audio.clone());
        let audio = audio.unwrap_or_else(crossbeam_channel::never);
        let held_check = recording.as_ref().map(|r| r.held_check.clone());
        let held_check = held_check.unwrap_or_else(crossbeam_channel::never);
        let unmute = unmute_at.map_or_else(crossbeam_channel::never, crossbeam_channel::at);
        let mut stop = false;
        crossbeam_channel::select! {
            recv(keys) -> key => match key {
                Ok(true) if recording.is_none() => {
                    recording = start_recording(shared);
                    // Only a new recording takes over the mute; a press
                    // that records nothing (paused) must not keep Me
                    // silent for good.
                    if recording.is_some() {
                        unmute_at = None;
                    }
                }
                Ok(false) => stop = true,
                Ok(true) => {}
                Err(_) => break,
            },
            recv(audio) -> packet => match (packet, recording.as_mut()) {
                (Ok(packet), Some(r)) => {
                    r.samples.extend(packet.samples);
                    let max = shared.config().max_seconds.max(1.0);
                    if r.samples.len() as f32 >= max * SAMPLE_RATE as f32 {
                        eprintln!("dictation: stopped at the {max:.0} s limit");
                        stop = true;
                    }
                }
                // The microphone went away: keep what it heard.
                (Err(_), Some(_)) => {
                    eprintln!("dictation: the microphone stopped");
                    stop = true;
                }
                (_, None) => {}
            },
            recv(held_check) -> _ => {
                let probe = shared.key_probe.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let (Some(probe), Some(r)) = (probe, recording.as_mut())
                    && missed_release(&mut r.seen_down, probe())
                {
                    eprintln!("dictation: the hotkey's release was missed; stopping");
                    stop = true;
                }
            },
            recv(unmute) -> _ => {
                shared.mute_me.store(false, Ordering::Relaxed);
                unmute_at = None;
            },
        }
        if stop && let Some(r) = recording.take() {
            stop_recording(shared, r, clips);
            unmute_at = Some(Instant::now() + UNMUTE_AFTER);
        }
    }
}

fn start_recording(shared: &Shared) -> Option<Recording> {
    let cfg = shared.config();
    let paused = shared.status().paused;
    if !cfg.enabled || paused {
        return None;
    }
    shared.mute_me.store(true, Ordering::Relaxed);
    let (tx, rx) = crossbeam_channel::unbounded();
    match start_mic(tx) {
        Ok(capture) => {
            if cfg.sounds {
                platform::play_sound(Sound::Start);
            }
            shared.update(|s| s.recording = true);
            Some(Recording {
                _capture: capture,
                audio: rx,
                samples: Vec::new(),
                held_check: crossbeam_channel::tick(HELD_CHECK),
                seen_down: false,
            })
        }
        Err(e) => {
            shared.mute_me.store(false, Ordering::Relaxed);
            shared.fail(format!("Could not start the microphone: {e:#}"), cfg.sounds);
            None
        }
    }
}

fn stop_recording(shared: &Shared, mut recording: Recording, clips: &Sender<Clip>) {
    let audio = recording.audio.clone();
    let mut samples = std::mem::take(&mut recording.samples);
    // Stops the microphone; what it had captured is still in the channel.
    drop(recording);
    samples.extend(audio.try_iter().flat_map(|p| p.samples));
    let cfg = shared.config();
    let seconds = samples.len() as f32 / SAMPLE_RATE as f32;
    let keep = seconds >= cfg.min_seconds;
    shared.update(|s| {
        s.recording = false;
        s.busy += usize::from(keep);
    });
    if keep {
        samples.truncate((cfg.max_seconds.max(1.0) * SAMPLE_RATE as f32) as usize);
        let at = now();
        // Saved now, not when its turn to be transcribed comes: a clip
        // waiting behind others (or for the model) survives a quit.
        let wav = cfg
            .save_recordings
            .then(|| archive::save_clip(&cfg.recordings_dir, at, &samples))
            .and_then(|saved| saved.map_err(|e| eprintln!("dictation: {e:#}")).ok());
        let _ = clips.send(Clip { samples, wav });
    } else {
        eprintln!("dictation: {seconds:.2} s is too short, ignored");
    }
}

/// Turns a clip into text and delivers it: beside its WAV, on the
/// clipboard, pasted.
fn finish(shared: &Shared, clip: Clip) {
    let cfg = shared.config();
    let Clip { samples, wav } = clip;
    let result = transcribe(shared, samples, &cfg);
    match result {
        Ok(Some(text)) => {
            let text = match &cfg.ollama {
                Some(o) => ollama::cleanup(&text, o),
                None => text,
            };
            if let Some(wav) = &wav
                && let Err(e) = archive::save_text(wav, &text)
            {
                eprintln!("dictation: {e:#}");
            }
            deliver(shared, &cfg, text);
        }
        Ok(None) => {
            eprintln!("dictation: nothing was recognized");
            if cfg.sounds {
                platform::play_sound(Sound::Error);
            }
        }
        Err(e) => shared.fail(format!("{e:#}"), cfg.sounds),
    }
    shared.update(|s| {
        s.busy = s.busy.saturating_sub(1);
        s.finished += 1;
    });
}

/// Copies the text, pastes it if asked, and remembers it.
fn deliver(shared: &Shared, cfg: &DictationConfig, text: String) {
    let delivered = platform::copy_text(&text).and_then(|()| {
        if cfg.auto_paste {
            platform::paste()?;
        }
        Ok(())
    });
    shared.update(|s| {
        s.history.push_front((now(), text));
        s.history.truncate(HISTORY);
    });
    match delivered {
        Ok(()) => {
            if cfg.sounds {
                platform::play_sound(Sound::Done);
            }
            shared.update(|s| s.last_error = None);
        }
        Err(e) => shared.fail(format!("{e:#}"), cfg.sounds),
    }
}

/// The clip's text, or None if nothing was said.
fn transcribe(shared: &Shared, samples: Vec<f32>, cfg: &DictationConfig) -> Result<Option<String>> {
    let jobs = wait_for_engine(shared)?;
    let (reply, result) = crossbeam_channel::bounded(1);
    jobs.send(Job::Dictation(DictationJob {
        samples,
        language: cfg.language.clone(),
        prompt: cfg.initial_prompt.clone(),
        reply,
    }))
    .map_err(|_| anyhow!("the transcriber stopped"))?;
    // Not kept: holding it would keep a replaced engine's worker alive.
    drop(jobs);
    match result.recv_timeout(RESULT_WAIT) {
        Ok(Output::Dictation(Ok(r))) => Ok(clean_text(&r.text)),
        Ok(Output::Dictation(Err(e))) => bail!("Transcription failed: {e}"),
        Ok(_) => bail!("unexpected transcriber output"),
        Err(RecvTimeoutError::Disconnected) => bail!("the transcriber stopped"),
        Err(RecvTimeoutError::Timeout) => bail!("Transcription did not finish in time"),
    }
}

/// The engine's job queue, once it is loaded. Asks the window to load the
/// model if none is.
fn wait_for_engine(shared: &Shared) -> Result<Sender<Job>> {
    let deadline = Instant::now() + ENGINE_WAIT;
    let mut engine = shared.engine.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        match &*engine {
            Engine::Ready(jobs) => return Ok(jobs.clone()),
            Engine::Unavailable(why) => bail!("{why}"),
            Engine::Missing => {
                shared.wants_engine.store(true, Ordering::Relaxed);
                (shared.wake)();
            }
            Engine::Loading => {}
        }
        let left = deadline
            .checked_duration_since(Instant::now())
            .context("The Whisper model did not load in time")?;
        engine = shared
            .engine_changed
            .wait_timeout(engine, left.min(Duration::from_secs(1)))
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

/// Whether this copy of the app may dictate.
#[derive(Debug)]
pub enum Claim {
    /// It holds the lock, for as long as this lives.
    Ours(std::fs::File),
    /// Another copy holds it: both would paste every dictation.
    Taken,
    /// No lock could be made (the folder is read-only, say); dictation goes
    /// ahead rather than staying off for good.
    Unchecked,
}

/// Claims dictation for this copy of the app with a lock on `path`. Copies
/// opened from the Dock are one app, but a copy run from elsewhere (a build
/// folder, a second install) is another.
pub fn claim(path: &Path) -> Claim {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path);
    let Ok(file) = file else { return Claim::Unchecked };
    match file.try_lock() {
        Ok(()) => Claim::Ours(file),
        Err(std::fs::TryLockError::WouldBlock) => Claim::Taken,
        Err(std::fs::TryLockError::Error(_)) => Claim::Unchecked,
    }
}

/// Whisper's text, tidied: one line of words, or None for silence markers
/// such as `[BLANK_AUDIO]`. Unlike meeting lines, short phrases like
/// "Thank you." are kept: when dictating they are meant.
pub fn clean_text(raw: &str) -> Option<String> {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let bracketed = |open: char, close: char| text.starts_with(open) && text.ends_with(close);
    if text.is_empty() || bracketed('[', ']') || bracketed('(', ')') || !text.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictation_text_keeps_short_phrases() {
        assert_eq!(clean_text("  Thank you. "), Some("Thank you.".into()));
        assert_eq!(clean_text(" 分批执行，\n然后 EP "), Some("分批执行， 然后 EP".into()));
        assert_eq!(clean_text("[BLANK_AUDIO]"), None);
        assert_eq!(clean_text(" (silence) "), None);
        assert_eq!(clean_text(" ... "), None);
        assert_eq!(clean_text(""), None);
    }

    #[test]
    fn phase_follows_voice2cursor_priorities() {
        let ready = Status {
            enabled: true,
            engine_ready: true,
            ..Status::default()
        };
        assert_eq!(ready.phase(), Phase::Ready);
        assert_eq!(
            Status {
                engine_ready: false,
                ..ready.clone()
            }
            .phase(),
            Phase::Loading
        );
        let failed = Status {
            last_error: Some((now(), "x".into())),
            ..ready.clone()
        };
        assert_eq!(failed.phase(), Phase::Error);
        let busy = Status { busy: 1, ..failed };
        assert_eq!(busy.phase(), Phase::Transcribing);
        let recording = Status {
            recording: true,
            ..busy
        };
        assert_eq!(recording.phase(), Phase::Recording);
        assert_eq!(
            Status {
                paused: true,
                ..recording
            }
            .phase(),
            Phase::Paused
        );
        assert_eq!(
            Status {
                enabled: false,
                ..ready
            }
            .phase(),
            Phase::Paused
        );
    }

    #[test]
    fn a_missed_release_needs_the_key_seen_down_first() {
        let mut seen = false;
        // A probe that can't see the key (no permission) never stops it.
        assert!(!missed_release(&mut seen, Some(false)));
        assert!(!missed_release(&mut seen, None));
        assert!(!missed_release(&mut seen, Some(true)));
        assert!(!missed_release(&mut seen, None));
        assert!(missed_release(&mut seen, Some(false)));
    }

    #[test]
    fn one_copy_of_the_app_dictates() {
        let path = std::env::temp_dir()
            .join(format!("meeting-transcriber-test-{}", std::process::id()))
            .join("dictation.lock");
        let first = claim(&path);
        assert!(matches!(first, Claim::Ours(_)), "{first:?}");
        assert!(matches!(claim(&path), Claim::Taken));
        drop(first);
        assert!(matches!(claim(&path), Claim::Ours(_)));
        assert!(matches!(claim(Path::new("/dev/null/x.lock")), Claim::Unchecked));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_clip_waits_for_the_engine_and_asks_for_it() {
        let dictation = Dictation::start(DictationConfig::default(), || {});
        let shared = dictation.shared.clone();
        let waiter = std::thread::spawn(move || wait_for_engine(&shared).map(|_| ()));
        while !dictation.take_engine_request() {
            std::thread::yield_now();
        }
        dictation.set_engine(Engine::Loading);
        let (jobs, _rx) = crossbeam_channel::unbounded();
        dictation.set_engine(Engine::Ready(jobs));
        assert!(waiter.join().unwrap().is_ok());
        assert!(dictation.status().engine_ready);

        dictation.set_engine(Engine::Unavailable("Download a model".into()));
        let err = wait_for_engine(&dictation.shared).unwrap_err();
        assert_eq!(err.to_string(), "Download a model");
        assert_eq!(dictation.status().phase(), Phase::Error);
    }
}
