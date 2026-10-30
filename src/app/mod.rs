//! The native window (egui): app state, background work, and the frame loop.
//! Pages live in their own modules.

mod library;
mod meeting_page;
mod platform;
mod reader;
mod settings_page;
mod sidebar;
mod theme;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use eframe::egui::{self, RichText};

use crate::chunker::ChunkerConfig;
use crate::models::{self, DEFAULT_MODEL, ModelInfo};
use crate::session::{Session, SessionConfig};
use crate::settings::{self, Settings};
use crate::transcriber::{Job, Output, SpeechToText, WhisperEngine, spawn_worker};
use crate::transcript::Transcript;
use library::{ExportPick, MeetingList, Notice, OpenMeeting};
use reader::Reader;

/// How long quitting waits for speech that was still being transcribed.
const EXIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(20);

/// Language choices: None means detect automatically.
const LANGUAGES: &[(Option<&str>, &str)] = &[
    (None, "Auto-detect"),
    (Some("en"), "English"),
    (Some("zh"), "Chinese"),
    (Some("ja"), "Japanese"),
    (Some("ko"), "Korean"),
    (Some("es"), "Spanish"),
    (Some("fr"), "French"),
    (Some("de"), "German"),
    (Some("pt"), "Portuguese"),
    (Some("ru"), "Russian"),
    (Some("hi"), "Hindi"),
];

fn language_name(code: Option<&str>) -> &'static str {
    LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map_or("Auto-detect", |(_, name)| name)
}

/// Sizes the way the model table states them (1 MB = 2^20 bytes).
fn format_size(bytes: u64) -> String {
    let mb = bytes as f64 / f64::from(1 << 20);
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Meeting,
    Settings,
}

/// The folders that can be chosen in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Folder {
    Models,
    Meetings,
}

struct Engine {
    model: ModelInfo,
    jobs: Sender<Job>,
    outputs: Receiver<Output>,
    ready: bool,
}

struct Download {
    model: ModelInfo,
    progress: Arc<Mutex<(u64, Option<u64>)>>,
    cancel: Arc<AtomicBool>,
    done: Receiver<anyhow::Result<PathBuf>>,
}

/// The native folder dialog, open on a background thread.
struct FolderPick {
    folder: Folder,
    result: Receiver<Option<PathBuf>>,
}

/// A new models folder was chosen while the old one still holds models.
struct MoveOffer {
    /// The new folder setting, applied whichever way the user answers.
    setting: String,
    to: PathBuf,
    count: usize,
    bytes: u64,
}

/// Models being moved to the models folder on a background thread.
struct ModelMove {
    from: PathBuf,
    progress: Arc<Mutex<(usize, usize)>>,
    done: Receiver<anyhow::Result<usize>>,
}

pub struct App {
    ctx: egui::Context,
    page: Page,
    model: ModelInfo,
    language: Option<&'static str>,
    capture_me: bool,
    capture_others: bool,
    /// Folder settings as stored: empty for the default.
    models_dir: String,
    meetings_dir: String,
    /// The old models folder of an unfinished move, as stored ("" if none).
    models_move_from: String,
    engine: Option<Engine>,
    /// Outputs of replaced engines. Their workers finish the jobs already
    /// queued (the last speech of a meeting) and then hang up.
    retired: Vec<Receiver<Output>>,
    download: Option<Download>,
    session: Option<Session>,
    transcript: Transcript,
    errors: Vec<String>,
    /// Whisper seconds per audio second for the last final chunk.
    speed: Option<f32>,
    next_utterance: u64,
    /// The meeting in the window; its lines are in `transcript`.
    open: OpenMeeting,
    /// The meeting file each session records into, by its first utterance
    /// id, so lines that finish after another meeting was opened still
    /// reach the right file.
    session_files: Vec<(u64, PathBuf)>,
    /// Such lines, waiting to be written.
    late_lines: Vec<(PathBuf, crate::transcript::Line)>,
    /// Meeting time at which the current session started.
    session_offset: f32,
    list: MeetingList,
    reader: Reader,
    export_pick: Option<ExportPick>,
    notice: Option<Notice>,
    saved_settings: Settings,
    /// The model whose Delete button was pressed once, awaiting confirmation.
    confirm_delete: Option<ModelInfo>,
    folder_pick: Option<FolderPick>,
    move_offer: Option<MoveOffer>,
    model_move: Option<ModelMove>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let settings = Settings::load();
        models::set_configured_dir(settings.models_dir_path());
        let move_from = settings.models_move_from_path();
        models::set_move_source(move_from.clone());
        // Nothing is downloading or moving yet, so these are left over
        // from a run that was cut short.
        for dir in [
            Some(models::models_dir()),
            Some(models::configured_dir()),
            move_from.clone(),
        ]
        .into_iter()
        .flatten()
        {
            models::remove_partial_files(&dir);
        }
        let mut transcript = Transcript::default();
        transcript.echo.enabled = settings.remove_echo;
        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            page: Page::Meeting,
            model: ModelInfo::by_id(&settings.model).unwrap_or(DEFAULT_MODEL),
            language: LANGUAGES
                .iter()
                .find(|(code, _)| code.is_some_and(|c| c == settings.language))
                .and_then(|(code, _)| *code),
            capture_me: settings.capture_me,
            capture_others: settings.capture_others,
            models_dir: settings.models_dir.clone(),
            meetings_dir: settings.meetings_dir.clone(),
            models_move_from: settings.models_move_from.clone(),
            engine: None,
            retired: Vec::new(),
            download: None,
            session: None,
            transcript,
            errors: Vec::new(),
            speed: None,
            next_utterance: 0,
            open: OpenMeeting::unsaved(),
            session_files: Vec::new(),
            late_lines: Vec::new(),
            session_offset: 0.0,
            list: MeetingList::default(),
            reader: Reader::default(),
            export_pick: None,
            notice: None,
            saved_settings: settings,
            confirm_delete: None,
            folder_pick: None,
            move_offer: None,
            model_move: None,
        };
        if let Some(from) = move_from {
            app.spawn_model_move(from);
        }
        app.ensure_engine();
        app
    }

    fn recording(&self) -> bool {
        self.session.is_some()
    }

    /// The selected model is loaded and can transcribe.
    fn engine_ready(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.ready && e.model == self.model)
    }

    /// Loads the selected model in the background if it is downloaded.
    fn ensure_engine(&mut self) {
        if self.engine.as_ref().is_some_and(|e| e.model == self.model) || !self.model.is_downloaded() {
            return;
        }
        self.retire_engine();
        let (job_tx, job_rx) = crossbeam_channel::unbounded();
        let (out_tx, out_rx) = crossbeam_channel::unbounded();
        let path = self.model.path();
        let ctx = self.ctx.clone();
        spawn_worker(
            move || Ok(Box::new(WhisperEngine::load(&path)?) as Box<dyn SpeechToText>),
            job_rx,
            out_tx,
            move || ctx.request_repaint(),
        );
        self.engine = Some(Engine {
            model: self.model,
            jobs: job_tx,
            outputs: out_rx,
            ready: false,
        });
    }

    /// Drops the engine without losing its work: dropping the job sender
    /// lets the worker finish what is queued and exit, and its outputs keep
    /// being read until then. Stopping a meeting and switching models (or
    /// folders) right away would otherwise lose the last lines.
    fn retire_engine(&mut self) {
        if let Some(Engine { outputs, ready, .. }) = self.engine.take()
            && ready
        {
            self.retired.push(outputs);
        }
    }

    /// Switches to another downloaded model (not while recording).
    fn use_model(&mut self, model: ModelInfo) {
        if self.recording() {
            return;
        }
        self.model = model;
        self.ensure_engine();
    }

    fn delete_model(&mut self, model: ModelInfo) {
        self.confirm_delete = None;
        if self.recording() && model == self.model {
            return;
        }
        if self.engine.as_ref().is_some_and(|e| e.model == model) {
            self.retire_engine();
        }
        if let Err(e) = model.delete() {
            self.errors.push(format!("{e:#}"));
        }
    }

    /// A download can start unless one is running or models are being
    /// moved (it would land in the folder being emptied).
    fn can_download(&self) -> bool {
        self.download.is_none() && self.model_move.is_none()
    }

    fn start_download(&mut self, model: ModelInfo) {
        if !self.can_download() {
            return;
        }
        let progress = Arc::new(Mutex::new((0, None)));
        let cancel = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let ctx = self.ctx.clone();
        let (p, c) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let result = models::download(model, c, |done, total| {
                *p.lock().unwrap() = (done, total);
                ctx.request_repaint_after(Duration::from_millis(200));
            });
            let _ = done_tx.send(result);
            ctx.request_repaint();
        });
        self.download = Some(Download {
            model,
            progress,
            cancel,
            done: done_rx,
        });
    }

    /// Progress bar and Cancel for the running download.
    fn download_progress(&self, ui: &mut egui::Ui) {
        let Some(d) = &self.download else { return };
        let (done, total) = *d.progress.lock().unwrap();
        let fraction = total.map_or(0.0, |t| done as f32 / t.max(1) as f32);
        ui.add(
            egui::ProgressBar::new(fraction)
                .desired_width(170.0)
                .corner_radius(6)
                .text(format!(
                    "{} / {} MB",
                    done >> 20,
                    total.map_or(u64::from(d.model.size_mb), |t| t >> 20)
                )),
        );
        if ui.button("Cancel").clicked() {
            d.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The folder the "Change…" dialog for `folder` should open in.
    fn folder_path(&self, folder: Folder) -> PathBuf {
        match folder {
            Folder::Models => models::configured_dir(),
            Folder::Meetings => self.meetings_dir_path(),
        }
    }

    fn meetings_dir_path(&self) -> PathBuf {
        settings::expand_dir(&self.meetings_dir).unwrap_or_else(crate::meetings::default_dir)
    }

    fn pick_folder(&mut self, folder: Folder) {
        let prompt = match folder {
            Folder::Models => "Choose a folder for Whisper models",
            Folder::Meetings => "Choose a folder for meeting transcripts",
        };
        let ctx = self.ctx.clone();
        let result = platform::pick_folder(prompt, &self.folder_path(folder), move || ctx.request_repaint());
        self.folder_pick = Some(FolderPick { folder, result });
    }

    /// Applies a folder setting ("" resets to the default). A new models
    /// folder first offers to bring the downloaded models along.
    fn set_folder(&mut self, folder: Folder, setting: String) {
        match folder {
            Folder::Meetings => self.meetings_dir = setting,
            // A running download writes into the current folder, so it would
            // end up left behind. The buttons are disabled meanwhile, but the
            // folder dialog may have been opened before the download started.
            Folder::Models if self.download.is_some() => self
                .errors
                .push("Finish or cancel the download before changing the models folder.".into()),
            Folder::Models => {
                let from = models::configured_dir();
                let to = settings::expand_dir(&setting).unwrap_or_else(models::default_models_dir);
                let found = models::downloaded_in(&from);
                if to == from || found.is_empty() {
                    self.apply_models_dir(setting);
                } else {
                    self.move_offer = Some(MoveOffer {
                        setting,
                        to,
                        count: found.len(),
                        bytes: found.iter().map(|(_, size)| size).sum(),
                    });
                }
            }
        }
    }

    fn apply_models_dir(&mut self, setting: String) {
        self.models_dir = setting;
        models::set_configured_dir(settings::expand_dir(&self.models_dir));
        // The loaded model can stay loaded only if it is in the new folder too.
        if !self.model.is_downloaded() && !self.recording() {
            self.retire_engine();
        }
        self.ensure_engine();
    }

    /// Switches to the offered folder and moves the models into it. The
    /// new folder and the old one are saved before anything moves, so if
    /// the app stops halfway the next launch finishes the move, and models
    /// not moved yet are still found meanwhile.
    fn start_model_move(&mut self, offer: MoveOffer) {
        let from = models::configured_dir();
        self.models_dir = offer.setting;
        self.models_move_from = from.to_string_lossy().into_owned();
        models::set_configured_dir(settings::expand_dir(&self.models_dir));
        models::set_move_source(Some(from.clone()));
        self.save_settings_if_changed();
        self.spawn_model_move(from);
    }

    /// Moves the models left in `from` into the models folder, in the
    /// background.
    fn spawn_model_move(&mut self, from: PathBuf) {
        let to = models::configured_dir();
        let count = models::downloaded_in(&from).len();
        let progress = Arc::new(Mutex::new((0, count)));
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let ctx = self.ctx.clone();
        let (p, src, dst) = (progress.clone(), from.clone(), to);
        std::thread::spawn(move || {
            let result = models::move_models(&src, &dst, |done, total| {
                *p.lock().unwrap() = (done, total);
                ctx.request_repaint();
            });
            let _ = done_tx.send(result);
            ctx.request_repaint();
        });
        self.model_move = Some(ModelMove {
            from,
            progress,
            done: done_rx,
        });
    }

    /// Ends an unfinished move: the old folder is no longer looked in.
    /// Models still there (if the user gave up on them) stay on disk.
    fn forget_model_move(&mut self) {
        self.models_move_from.clear();
        models::set_move_source(None);
        self.save_settings_if_changed();
        // The loaded model can stay loaded only if it is still found.
        if !self.model.is_downloaded() && !self.recording() {
            self.retire_engine();
        }
        self.ensure_engine();
    }

    fn start_session(&mut self) {
        if self.open.read_only() || !self.engine.as_ref().is_some_and(|e| e.ready) {
            return;
        }
        // The file must exist before recording, so lines that finish after
        // the user moves on to another meeting have somewhere to go.
        self.save_open_creating();
        let Some(engine) = self.engine.as_ref() else { return };
        // A reopened meeting continues its clock.
        let time_offset = self
            .transcript
            .lines()
            .last()
            .map_or(0.0, |l| l.end.ceil())
            .max(self.open.meeting.duration as f32);
        let cfg = SessionConfig {
            capture_me: self.capture_me,
            capture_others: self.capture_others,
            chunker: ChunkerConfig::default(),
            time_offset,
            first_utterance: self.next_utterance,
            language: self.language.map(str::to_string),
        };
        let first_utterance = self.next_utterance;
        self.next_utterance += 10_000_000;
        let ctx = self.ctx.clone();
        match Session::start(cfg, engine.jobs.clone(), move || ctx.request_repaint()) {
            Ok(session) => {
                self.errors.extend(session.warnings.iter().cloned());
                if let Some(path) = &self.open.path {
                    self.session_files.push((first_utterance, path.clone()));
                }
                self.session_offset = time_offset;
                self.session = Some(session);
            }
            Err(e) => self.errors.push(format!("{e:#}")),
        }
    }

    fn stop_session(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.open.meeting.duration = self.open.meeting.duration.max(self.recorded_seconds());
        if let Some(session) = self.session.take() {
            session.stop();
        }
        self.mark_edited();
        self.save_open();
    }

    /// Collects results from background threads.
    fn poll(&mut self) {
        self.poll_background();
        self.poll_export_pick();
        self.poll_engine();
        self.flush_late_lines();
    }

    fn poll_background(&mut self) {
        if let Some(d) = &self.download
            && let Ok(result) = d.done.try_recv()
        {
            let model = d.model;
            self.download = None;
            match result {
                Ok(_) if model == self.model => self.ensure_engine(),
                Ok(_) => {}
                Err(e) => self.errors.push(format!("Download of {} failed: {e:#}", model.label)),
            }
        }

        if let Some(pick) = &self.folder_pick
            && let Ok(chosen) = pick.result.try_recv()
        {
            let folder = pick.folder;
            self.folder_pick = None;
            if let Some(path) = chosen {
                self.set_folder(folder, path.to_string_lossy().into_owned());
            }
        }

        if let Some(m) = &self.model_move
            && let Ok(result) = m.done.try_recv()
        {
            let from = m.from.clone();
            self.model_move = None;
            match result {
                Ok(_) => self.forget_model_move(),
                // Kept as unfinished: still used from there, and retried
                // from Settings or on the next launch.
                Err(e) => self.errors.push(format!(
                    "Some models could not be moved and are still in {}: {e:#}",
                    from.display()
                )),
            }
        }
    }

    fn poll_engine(&mut self) {
        let mut changed = false;
        let mut failed = false;
        let outputs: Vec<Output> = self
            .engine
            .as_ref()
            .map_or_else(Vec::new, |e| e.outputs.try_iter().collect());
        for output in outputs {
            match output {
                Output::Ready => {
                    if let Some(engine) = &mut self.engine {
                        engine.ready = true;
                    }
                }
                Output::LoadFailed(e) => {
                    let label = self.engine.as_ref().map_or("the model", |e| e.model.label);
                    self.errors.push(format!("Could not load {label}: {e}"));
                    failed = true;
                }
                other => changed |= self.apply_output(other),
            }
        }
        if failed {
            self.engine = None;
        }
        changed |= self.poll_retired();
        if changed {
            self.mark_new_lines();
        }
    }

    /// Reads what retired engines still produce, forgetting those whose
    /// worker has exited.
    fn poll_retired(&mut self) -> bool {
        let mut changed = false;
        let mut retired = std::mem::take(&mut self.retired);
        retired.retain(|outputs| {
            loop {
                match outputs.try_recv() {
                    Ok(output) => changed |= self.apply_output(output),
                    Err(TryRecvError::Empty) => break true,
                    Err(TryRecvError::Disconnected) => break false,
                }
            }
        });
        self.retired = retired;
        changed
    }

    /// Applies a transcription result. Returns true if a line was added to
    /// the open meeting.
    fn apply_output(&mut self, output: Output) -> bool {
        match output {
            Output::Preview { utterance, .. }
            | Output::Final {
                utterance, line: None, ..
            } if !self.for_open_meeting(utterance) => {}
            Output::Final {
                utterance,
                line: Some(line),
                ..
            } if !self.for_open_meeting(utterance) => self.add_late_line(utterance, line),
            Output::Preview {
                speaker,
                utterance,
                start,
                text,
            } => self.transcript.set_preview(speaker, utterance, start, text),
            Output::Final {
                speaker,
                utterance,
                line,
            } => match line {
                Some(line) => return self.transcript.add_final(utterance, line),
                None => self.transcript.end_utterance(speaker, utterance),
            },
            Output::Speed { audio_seconds, seconds } => {
                if audio_seconds > 0.0 {
                    self.speed = Some(seconds / audio_seconds);
                }
            }
            Output::Error(e) => self.errors.push(e),
            // Only meaningful for the current engine; see poll_engine.
            Output::Ready | Output::LoadFailed(_) => {}
        }
        false
    }

    /// On quit: waits (up to EXIT_DRAIN_TIMEOUT) for speech that was still
    /// being transcribed, so the saved file has the meeting's last lines.
    /// Returns at once when nothing is pending, since an idle worker exits
    /// as soon as its job sender is gone.
    fn finish_pending(&mut self) {
        self.retire_engine();
        let deadline = Instant::now() + EXIT_DRAIN_TIMEOUT;
        let mut changed = false;
        for outputs in std::mem::take(&mut self.retired) {
            // Ends when the worker hangs up or time runs out.
            while let Ok(output) = outputs.recv_deadline(deadline) {
                changed |= self.apply_output(output);
            }
        }
        if changed {
            self.mark_new_lines();
        }
        self.flush_late_lines();
        self.save_open();
    }

    fn settings(&self) -> Settings {
        Settings {
            model: self.model.id.to_string(),
            language: self.language.unwrap_or_default().to_string(),
            capture_me: self.capture_me,
            capture_others: self.capture_others,
            remove_echo: self.transcript.echo.enabled,
            models_dir: self.models_dir.clone(),
            meetings_dir: self.meetings_dir.clone(),
            models_move_from: self.models_move_from.clone(),
        }
    }

    fn save_settings_if_changed(&mut self) {
        let settings = self.settings();
        if settings != self.saved_settings {
            settings.save();
            self.saved_settings = settings;
        }
    }

    fn error_banner(&mut self, ui: &mut egui::Ui) {
        let mut dismiss = None;
        for (i, e) in self.errors.iter().enumerate() {
            let red = ui.visuals().error_fg_color;
            egui::Frame::new()
                .fill(red.gamma_multiply(0.12))
                .stroke(egui::Stroke::new(1.0, red.gamma_multiply(0.4)))
                .inner_margin(egui::Margin::symmetric(12, 8))
                .corner_radius(8)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(e).color(red));
                        if ui.small_button("Dismiss").clicked() {
                            dismiss = Some(i);
                        }
                    });
                });
            ui.add_space(8.0);
        }
        if let Some(i) = dismiss {
            self.errors.remove(i);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        self.refresh_library();
        self.autosave();
        self.save_settings_if_changed();

        egui::Panel::left("sidebar")
            .resizable(false)
            .exact_size(232.0)
            .show_separator_line(false)
            .frame(theme::sidebar_frame(ui))
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(theme::content_frame(ui))
            .show(ui, |ui| {
                self.error_banner(ui);
                match self.page {
                    Page::Meeting => self.meeting_page(ui),
                    Page::Settings => self.settings_page(ui),
                }
            });

        if self.recording() {
            // Keep the meters and clock moving.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self) {
        self.stop_session();
        self.finish_pending();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_the_model_table() {
        assert_eq!(format_size(142 << 20), "142 MB");
        assert_eq!(format_size(547 << 20), "547 MB");
        assert_eq!(format_size(1550 << 20), "1.5 GB");
        assert_eq!(format_size(0), "0 MB");
    }

    #[test]
    fn language_names() {
        assert_eq!(language_name(None), "Auto-detect");
        assert_eq!(language_name(Some("zh")), "Chinese");
        assert_eq!(language_name(Some("xx")), "Auto-detect");
    }
}
