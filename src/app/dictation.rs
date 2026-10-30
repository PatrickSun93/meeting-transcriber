//! Dictation in the window: starting the hotkey watcher, handing the
//! dictation threads the shared engine and the settings, the menu bar item,
//! closing to the menu bar, and the recordings list for the sidebar.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;

use super::App;
use crate::dictation::archive::{self, Clip};
use crate::dictation::{Dictation, DictationConfig, Engine, Phase, Status, voice2cursor};
use crate::platform::{self, HotkeyWatch, KeyEvent, TrayAction, TrayIcon, TrayMenu};
use crate::settings::DictationSettings;

/// How often the recordings folder is listed again while it is shown.
const RESCAN_EVERY: Duration = Duration::from_secs(5);
/// If the window has not closed this long after Quit, the app saves and
/// exits by itself.
const QUIT_GRACE: Duration = Duration::from_secs(3);

/// The recordings folder, listed in the background.
#[derive(Default)]
pub(super) struct ClipList {
    pub clips: Vec<Clip>,
    dir: PathBuf,
    scanned: Option<Instant>,
    /// `Status::finished` when last listed: a new clip lists again.
    finished: u64,
    scanning: Option<Receiver<Vec<Clip>>>,
}

pub(super) struct DictationState {
    pub dictation: Dictation,
    pub settings: DictationSettings,
    /// What the threads were last given.
    pushed: DictationConfig,
    pub paused: bool,
    pushed_paused: bool,
    pub hotkey: Option<HotkeyWatch>,
    /// Why the hotkey can't be watched, until dismissed.
    pub hotkey_error: Option<String>,
    key_events: (Sender<KeyEvent>, Receiver<KeyEvent>),
    /// voice2cursor was running, so dictation started paused.
    pub voice2cursor_banner: bool,
    /// The engine as last told to the dictation threads.
    engine_key: String,
    /// Why the engine failed to load, until another load starts.
    pub engine_failed: Option<String>,
    tray: Option<platform::Tray>,
    tray_tried: bool,
    tray_actions: (Sender<TrayAction>, Receiver<TrayAction>),
    /// Hide the window once it has been shown (opened at login).
    start_hidden: bool,
    frames: u64,
    pub window_hidden: bool,
    quit_at: Option<Instant>,
    pub clips: ClipList,
    /// The clip open on the Dictation page, by file stem.
    pub selected: Option<String>,
    /// How many clips the sidebar lists.
    pub list_limit: usize,
    pub open_at_login: bool,
}

impl DictationState {
    pub fn new(settings: DictationSettings, ctx: &egui::Context, start_hidden: bool) -> Self {
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let config = settings.config();
        let dictation = Dictation::start(config.clone(), wake);
        // Both would paste every dictation.
        let voice2cursor = voice2cursor::is_running();
        dictation.set_paused(voice2cursor);
        Self {
            dictation,
            settings,
            pushed: config,
            paused: voice2cursor,
            pushed_paused: voice2cursor,
            hotkey: None,
            hotkey_error: None,
            key_events: crossbeam_channel::unbounded(),
            voice2cursor_banner: voice2cursor,
            engine_key: String::new(),
            engine_failed: None,
            tray: None,
            tray_tried: false,
            tray_actions: crossbeam_channel::unbounded(),
            start_hidden,
            frames: 0,
            window_hidden: false,
            quit_at: None,
            clips: ClipList::default(),
            selected: None,
            list_limit: super::dictation_page::LIST_STEP,
            open_at_login: platform::open_at_login(),
        }
    }

    pub fn status(&self) -> Status {
        self.dictation.status()
    }

    /// Whether closing the window hides it instead of quitting.
    fn closes_to_tray(&self) -> bool {
        self.tray.is_some() && self.quit_at.is_none()
    }
}

/// One line about dictation's state, for the menu and the Dictation page.
pub(super) fn describe(status: &Status, hotkey: &str) -> String {
    match status.phase() {
        Phase::Paused if !status.enabled => "Dictation is off".into(),
        Phase::Paused => "Dictation is paused".into(),
        Phase::Recording => "Recording… let go to transcribe".into(),
        Phase::Transcribing => "Transcribing…".into(),
        Phase::Error => match (&status.engine_problem, &status.last_error) {
            (Some(why), _) => why.clone(),
            (None, Some((at, why))) => format!("{}: {why}", at.format("%H:%M")),
            (None, None) => "Something went wrong".into(),
        },
        Phase::Loading => "Loading the Whisper model…".into(),
        Phase::Ready => format!("Ready: hold {hotkey} to dictate"),
    }
}

fn tray_icon(phase: Phase) -> TrayIcon {
    match phase {
        Phase::Loading => TrayIcon::Loading,
        Phase::Paused => TrayIcon::Paused,
        Phase::Recording => TrayIcon::Recording,
        Phase::Transcribing => TrayIcon::Transcribing,
        Phase::Error => TrayIcon::Error,
        Phase::Ready => TrayIcon::Ready,
    }
}

impl App {
    /// Starts watching the hotkey. Without permission, says how to get it.
    pub(super) fn start_hotkey(&mut self) {
        if self.dict.hotkey.is_some() {
            return;
        }
        let keys = self.dict.dictation.key_sender();
        let events = self.dict.key_events.0.clone();
        let ctx = self.ctx.clone();
        let result = platform::watch_hotkey(self.dict.settings.hotkey(), move |event| match event {
            KeyEvent::Down => keys(true),
            KeyEvent::Up => keys(false),
            other => {
                let _ = events.send(other);
                ctx.request_repaint();
            }
        });
        match result {
            Ok(watch) => {
                self.dict.hotkey = Some(watch);
                self.dict.hotkey_error = None;
            }
            Err(e) => self.dict.hotkey_error = Some(format!("{e:#} {}", platform::HOTKEY_HINT).trim().to_string()),
        }
    }

    /// Pauses or resumes. Resuming while voice2cursor still runs keeps it
    /// paused and says why again.
    pub(super) fn set_dictation_paused(&mut self, paused: bool) {
        if !paused && voice2cursor::is_running() {
            self.dict.voice2cursor_banner = true;
            return;
        }
        self.dict.paused = paused;
        if !paused {
            self.dict.voice2cursor_banner = false;
        }
    }

    /// Runs every frame, also while the window is hidden.
    pub(super) fn dictation_logic(&mut self, ctx: &egui::Context) {
        self.dict.frames += 1;
        self.make_tray();
        self.poll_key_events();
        self.poll_tray_actions(ctx);
        self.sync_dictation_engine();
        self.push_dictation_settings();
        self.update_tray();
        self.close_to_tray(ctx);
        if let Some(at) = self.dict.quit_at {
            if at.elapsed() > QUIT_GRACE {
                // The window did not close (it may be hidden): save and go.
                self.stop_session();
                self.finish_pending();
                std::process::exit(0);
            }
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// The menu bar item, made once the app is running.
    fn make_tray(&mut self) {
        if self.dict.tray_tried {
            return;
        }
        self.dict.tray_tried = true;
        let ctx = self.ctx.clone();
        match platform::Tray::new(
            self.dict.tray_actions.0.clone(),
            Box::new(move || ctx.request_repaint()),
        ) {
            Ok(tray) => self.dict.tray = Some(tray),
            Err(e) => {
                if platform::HAS_TRAY {
                    self.errors.push(format!("{e:#}"));
                }
                // Nowhere to come back from: show the window after all.
                self.dict.start_hidden = false;
            }
        }
    }

    fn poll_key_events(&mut self) {
        let events: Vec<KeyEvent> = self.dict.key_events.1.try_iter().collect();
        for event in events {
            match event {
                KeyEvent::Captured(key) => self.dict.settings.hotkey = key.name(),
                KeyEvent::CaptureCancelled | KeyEvent::Down | KeyEvent::Up => {}
            }
        }
    }

    /// Whether the next key press is being captured as the hotkey.
    pub(super) fn capturing_hotkey(&self) -> bool {
        self.dict.hotkey.as_ref().is_some_and(HotkeyWatch::capturing)
    }

    fn poll_tray_actions(&mut self, ctx: &egui::Context) {
        let actions: Vec<TrayAction> = self.dict.tray_actions.1.try_iter().collect();
        for action in actions {
            match action {
                TrayAction::Copy(text) => {
                    if let Err(e) = platform::copy_text(&text) {
                        self.errors.push(format!("{e:#}"));
                    }
                }
                TrayAction::TogglePause => self.set_dictation_paused(!self.dict.paused),
                TrayAction::OpenWindow => self.show_window(ctx),
                TrayAction::Quit => {
                    self.dict.quit_at = Some(Instant::now());
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    pub(super) fn show_window(&mut self, ctx: &egui::Context) {
        platform::show_app();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.dict.window_hidden = false;
    }

    fn hide_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        platform::hide_app();
        self.dict.window_hidden = true;
    }

    /// With a menu bar item, the close button hides the window; dictation
    /// and any recording carry on. Quit in the menu (or ⌘Q) really quits.
    fn close_to_tray(&mut self, ctx: &egui::Context) {
        if self.dict.start_hidden && self.dict.tray.is_some() && self.dict.frames >= 3 {
            self.dict.start_hidden = false;
            self.hide_window(ctx);
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.dict.closes_to_tray() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            // Unsaved notes are written now rather than at quit.
            self.save_open();
            self.hide_window(ctx);
        }
    }

    /// Tells the dictation threads about the engine whenever it changes.
    fn sync_dictation_engine(&mut self) {
        if self.dict.dictation.take_engine_request() && self.engine.is_none() && self.dict.engine_failed.is_none() {
            self.ensure_engine();
        }
        let (key, engine) = match &self.engine {
            Some(e) if e.ready => (format!("ready {}", e.model.id), Engine::Ready(e.jobs.clone())),
            Some(e) => (format!("loading {}", e.model.id), Engine::Loading),
            None => match (&self.dict.engine_failed, self.model.is_downloaded()) {
                (Some(why), _) => (format!("failed {why}"), Engine::Unavailable(why.clone())),
                (None, false) => (
                    "no model".into(),
                    Engine::Unavailable(format!(
                        "No Whisper model is downloaded. Download {} in Settings to dictate.",
                        self.model.label
                    )),
                ),
                (None, true) => ("missing".into(), Engine::Missing),
            },
        };
        if key != self.dict.engine_key {
            self.dict.engine_key = key;
            self.dict.dictation.set_engine(engine);
        }
    }

    /// Called when the engine is dropped: dictation must not keep its job
    /// queue, or the old worker would never exit.
    pub(super) fn forget_dictation_engine(&mut self) {
        self.dict.engine_key.clear();
        self.dict.dictation.set_engine(Engine::Missing);
    }

    fn push_dictation_settings(&mut self) {
        let config = self.dict.settings.config();
        if config != self.dict.pushed {
            self.dict.dictation.set_config(config.clone());
            self.dict.pushed = config;
        }
        if self.dict.paused != self.dict.pushed_paused {
            self.dict.dictation.set_paused(self.dict.paused);
            self.dict.pushed_paused = self.dict.paused;
        }
        let key = self.dict.settings.hotkey();
        if let Some(watch) = &self.dict.hotkey
            && watch.key() != key
        {
            watch.set_key(key);
        }
    }

    fn update_tray(&mut self) {
        let Some(tray) = &self.dict.tray else { return };
        let status = self.dict.status();
        let text = describe(&status, &self.dict.settings.hotkey().label());
        let menu = TrayMenu {
            status: text.clone(),
            recent: status
                .history
                .iter()
                .map(|(at, text)| (at.format("%H:%M").to_string(), text.clone()))
                .collect(),
            paused: self.dict.paused,
        };
        tray.set(
            tray_icon(status.phase()),
            &format!("Meeting Transcriber · {text}"),
            menu,
        );
    }

    /// Lists the recordings folder again when it changed, a clip finished,
    /// or a while has passed. Only while the list is on screen.
    pub(super) fn refresh_clips(&mut self) {
        let list = &mut self.dict.clips;
        if let Some(rx) = &list.scanning
            && let Ok(clips) = rx.try_recv()
        {
            list.clips = clips;
            list.scanning = None;
        }
        let dir = self.dict.settings.recordings_dir_path();
        let finished = self.dict.dictation.status().finished;
        let due = list.scanned.is_none_or(|t| t.elapsed() >= RESCAN_EVERY);
        if list.scanning.is_none() && (due || dir != list.dir || finished != list.finished) {
            if dir != list.dir {
                list.clips.clear();
            }
            let (tx, rx) = crossbeam_channel::bounded(1);
            let ctx = self.ctx.clone();
            let scan_dir = dir.clone();
            std::thread::spawn(move || {
                let _ = tx.send(archive::list(&scan_dir));
                ctx.request_repaint();
            });
            list.scanning = Some(rx);
            list.dir = dir;
            list.finished = finished;
            list.scanned = Some(Instant::now());
        }
        self.ctx.request_repaint_after(RESCAN_EVERY);
    }
}
