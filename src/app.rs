//! The native window (egui).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, Color32, RichText};

use crate::chunker::ChunkerConfig;
use crate::models::{self, DEFAULT_MODEL, MODELS, ModelInfo};
use crate::session::{Session, SessionConfig};
use crate::settings::Settings;
use crate::transcriber::{Job, Output, SpeechToText, WhisperEngine, spawn_worker};
use crate::transcript::{Speaker, Transcript, format_time};

const ME_COLOR: Color32 = Color32::from_rgb(70, 140, 230);
const OTHERS_COLOR: Color32 = Color32::from_rgb(230, 140, 50);

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

fn speaker_color(s: Speaker) -> Color32 {
    match s {
        Speaker::Me => ME_COLOR,
        Speaker::Others => OTHERS_COLOR,
    }
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

pub struct App {
    ctx: egui::Context,
    model: ModelInfo,
    language: Option<&'static str>,
    capture_me: bool,
    capture_others: bool,
    engine: Option<Engine>,
    download: Option<Download>,
    session: Option<Session>,
    transcript: Transcript,
    errors: Vec<String>,
    /// Whisper seconds per audio second for the last final chunk.
    speed: Option<f32>,
    next_utterance: u64,
    /// Where the transcript is saved; chosen when the first session starts.
    save_path: Option<PathBuf>,
    saved_settings: Settings,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fallback_fonts(&cc.egui_ctx);
        let settings = Settings::load();
        let mut transcript = Transcript::default();
        transcript.echo.enabled = settings.remove_echo;
        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            model: ModelInfo::by_id(&settings.model).unwrap_or(DEFAULT_MODEL),
            language: LANGUAGES
                .iter()
                .find(|(code, _)| code.is_some_and(|c| c == settings.language))
                .and_then(|(code, _)| *code),
            capture_me: settings.capture_me,
            capture_others: settings.capture_others,
            engine: None,
            download: None,
            session: None,
            transcript,
            errors: Vec::new(),
            speed: None,
            next_utterance: 0,
            save_path: None,
            saved_settings: settings,
        };
        app.ensure_engine();
        app
    }

    /// Loads the selected model in the background if it is downloaded.
    fn ensure_engine(&mut self) {
        if self.engine.as_ref().is_some_and(|e| e.model == self.model) || !self.model.is_downloaded() {
            return;
        }
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
        // Dropping the old engine's job sender lets its worker finish and exit.
        self.engine = Some(Engine {
            model: self.model,
            jobs: job_tx,
            outputs: out_rx,
            ready: false,
        });
    }

    fn start_download(&mut self) {
        let model = self.model;
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

    fn start_session(&mut self) {
        let Some(engine) = self.engine.as_ref().filter(|e| e.ready) else {
            return;
        };
        if self.save_path.is_none() {
            self.save_path = Some(new_save_path());
        }
        let time_offset = self.transcript.lines().last().map_or(0.0, |l| l.end.ceil());
        let cfg = SessionConfig {
            capture_me: self.capture_me,
            capture_others: self.capture_others,
            chunker: ChunkerConfig::default(),
            time_offset,
            first_utterance: self.next_utterance,
            language: self.language.map(str::to_string),
        };
        self.next_utterance += 10_000_000;
        let ctx = self.ctx.clone();
        match Session::start(cfg, engine.jobs.clone(), move || ctx.request_repaint()) {
            Ok(session) => {
                self.errors.extend(session.warnings.iter().cloned());
                self.session = Some(session);
            }
            Err(e) => self.errors.push(format!("{e:#}")),
        }
    }

    fn stop_session(&mut self) {
        if let Some(session) = self.session.take() {
            session.stop();
        }
    }

    fn poll(&mut self) {
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

        let mut changed = false;
        let mut failed = false;
        if let Some(engine) = &mut self.engine {
            for output in engine.outputs.try_iter() {
                match output {
                    Output::Ready => engine.ready = true,
                    Output::LoadFailed(e) => {
                        self.errors.push(format!("Could not load {}: {e}", engine.model.label));
                        failed = true;
                    }
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
                        Some(line) => changed |= self.transcript.add_final(utterance, line),
                        None => self.transcript.end_utterance(speaker, utterance),
                    },
                    Output::Speed { audio_seconds, seconds } => {
                        if audio_seconds > 0.0 {
                            self.speed = Some(seconds / audio_seconds);
                        }
                    }
                    Output::Error(e) => self.errors.push(e),
                }
            }
        }
        if failed {
            self.engine = None;
        }
        if changed {
            self.save();
        }
    }

    fn save(&mut self) {
        let Some(path) = &self.save_path else { return };
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(path, self.transcript.to_text()));
        if let Err(e) = result {
            self.errors.push(format!("Could not save {}: {e}", path.display()));
            self.save_path = None;
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let recording = self.session.is_some();
        let ready = self.engine.as_ref().is_some_and(|e| e.ready && e.model == self.model);

        ui.horizontal(|ui| {
            if recording {
                if ui.button(RichText::new("■  Stop").size(18.0)).clicked() {
                    self.stop_session();
                }
            } else {
                let start = ui.add_enabled(ready, egui::Button::new(RichText::new("●  Start").size(18.0)));
                if start.clicked() {
                    self.start_session();
                }
            }
            ui.add_space(8.0);
            self.status(ui);
        });

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!recording && self.download.is_none(), |ui| {
                ui.label("Model");
                let before = self.model;
                egui::ComboBox::from_id_salt("model")
                    .selected_text(self.model.label)
                    .show_ui(ui, |ui| {
                        for m in MODELS {
                            let label = format!(
                                "{}  ({} MB{})",
                                m.label,
                                m.size_mb,
                                if m.is_downloaded() { "" } else { ", download" }
                            );
                            ui.selectable_value(&mut self.model, *m, label);
                        }
                    });
                if before != self.model {
                    self.ensure_engine();
                }

                ui.label("Language");
                let current = LANGUAGES
                    .iter()
                    .find(|(code, _)| *code == self.language)
                    .map_or("Auto-detect", |(_, name)| name);
                egui::ComboBox::from_id_salt("language")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (code, name) in LANGUAGES {
                            ui.selectable_value(&mut self.language, *code, *name);
                        }
                    });

                ui.checkbox(&mut self.capture_me, "Me (microphone)");
                ui.checkbox(&mut self.capture_others, "Others (Mac audio)");
            });
            ui.checkbox(&mut self.transcript.echo.enabled, "Remove speaker echo")
                .on_hover_text(
                    "When the meeting plays through speakers, your mic hears the others too. \
                     This drops your lines that repeat what Others just said. \
                     Not needed with headphones.",
                );
        });

        if let Some(session) = &self.session {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for speaker in [Speaker::Me, Speaker::Others] {
                    let level = (session.levels.get(speaker) * 8.0).clamp(0.0, 1.0);
                    ui.colored_label(speaker_color(speaker), speaker.label());
                    ui.add(
                        egui::ProgressBar::new(level)
                            .desired_width(120.0)
                            .fill(speaker_color(speaker)),
                    );
                    ui.add_space(12.0);
                }
            });
        }
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        if let Some(d) = &self.download {
            let (done, total) = *d.progress.lock().unwrap();
            let fraction = total.map_or(0.0, |t| done as f32 / t.max(1) as f32);
            ui.label(format!("Downloading {}…", d.model.label));
            ui.add(egui::ProgressBar::new(fraction).desired_width(160.0).text(format!(
                "{} / {} MB",
                done >> 20,
                total.map_or(d.model.size_mb as u64, |t| t >> 20)
            )));
            if ui.button("Cancel").clicked() {
                d.cancel.store(true, Ordering::Relaxed);
            }
            return;
        }
        if !self.model.is_downloaded() {
            ui.label(format!("{} is not downloaded yet.", self.model.label));
            if ui.button(format!("Download ({} MB)", self.model.size_mb)).clicked() {
                self.start_download();
            }
            return;
        }
        match &self.engine {
            Some(e) if !e.ready => {
                ui.spinner();
                ui.label("Loading model…");
            }
            None => {
                if ui.button("Load model").clicked() {
                    self.ensure_engine();
                }
            }
            Some(_) => {
                if let Some(session) = &self.session {
                    let t = session.started.elapsed().as_secs_f32();
                    ui.label(RichText::new(format!("Recording  {}", format_time(t))).color(Color32::RED));
                    ui.weak(session.source_names.join(" + "));
                } else {
                    ui.label("Ready");
                }
                if let Some(speed) = self.speed {
                    ui.weak(format!("· Whisper {speed:.2}× real time"))
                        .on_hover_text("Seconds of processing per second of speech. Below 1 keeps up live.");
                }
            }
        }
    }

    fn transcript_view(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.transcript.is_empty()
                    && self.transcript.preview(Speaker::Me).is_none()
                    && self.transcript.preview(Speaker::Others).is_none()
                {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.weak("Press Start, then join or continue your meeting.");
                        ui.weak("Your microphone shows as Me; everything the Mac plays shows as Others.");
                    });
                }
                for line in self.transcript.lines() {
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(format_time(line.start));
                        ui.label(
                            RichText::new(line.speaker.label())
                                .strong()
                                .color(speaker_color(line.speaker)),
                        );
                        ui.add(egui::Label::new(&line.text).selectable(true));
                    });
                }
                for speaker in [Speaker::Me, Speaker::Others] {
                    if let Some(p) = self.transcript.preview(speaker) {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(format_time(p.start));
                            ui.label(RichText::new(speaker.label()).color(speaker_color(speaker)));
                            ui.label(RichText::new(&p.text).italics().weak());
                        });
                    }
                }
            });
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let has_lines = !self.transcript.is_empty();
            if ui.add_enabled(has_lines, egui::Button::new("Copy all")).clicked() {
                ui.ctx().copy_text(self.transcript.to_text());
            }
            if let Some(path) = self.save_path.clone() {
                if ui
                    .button("Show file")
                    .on_hover_text(path.display().to_string())
                    .clicked()
                {
                    reveal_in_finder(&path);
                }
                ui.weak(format!(
                    "Saving to {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            if ui
                .add_enabled(has_lines && self.session.is_none(), egui::Button::new("New transcript"))
                .on_hover_text("Clears the window. The saved file is kept.")
                .clicked()
            {
                self.transcript.clear();
                self.save_path = None;
            }
            if self.transcript.echo_removed > 0 {
                ui.weak(format!("{} echo lines removed", self.transcript.echo_removed));
            }
        });
    }

    fn error_banner(&mut self, ui: &mut egui::Ui) {
        let mut dismiss = None;
        for (i, e) in self.errors.iter().enumerate() {
            egui::Frame::new()
                .fill(ui.visuals().error_fg_color.gamma_multiply(0.15))
                .inner_margin(6.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                        if ui.small_button("Dismiss").clicked() {
                            dismiss = Some(i);
                        }
                    });
                });
        }
        if let Some(i) = dismiss {
            self.errors.remove(i);
        }
    }
}

impl App {
    fn settings(&self) -> Settings {
        Settings {
            model: self.model.id.to_string(),
            language: self.language.unwrap_or_default().to_string(),
            capture_me: self.capture_me,
            capture_others: self.capture_others,
            remove_echo: self.transcript.echo.enabled,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let settings = self.settings();
        if settings != self.saved_settings {
            settings.save();
            self.saved_settings = settings;
        }
        egui::Panel::top("controls").show(ui, |ui| {
            ui.add_space(6.0);
            self.controls(ui);
            self.error_banner(ui);
            ui.add_space(6.0);
        });
        egui::Panel::bottom("footer").show(ui, |ui| {
            ui.add_space(4.0);
            self.footer(ui);
            ui.add_space(4.0);
        });
        egui::CentralPanel::default().show(ui, |ui| self.transcript_view(ui));

        if self.session.is_some() {
            // Keep the meters and clock moving.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self) {
        self.stop_session();
    }
}

/// `~/Documents/Meeting Transcripts/Meeting 2026-10-29 14-05.txt`
fn new_save_path() -> PathBuf {
    let dir = dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
        .join("Meeting Transcripts");
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M");
    let mut path = dir.join(format!("Meeting {stamp}.txt"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Meeting {stamp} ({n}).txt"));
        n += 1;
    }
    path
}

fn reveal_in_finder(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(target_os = "macos"))]
    if let Some(dir) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// egui's built-in fonts have no Chinese, Japanese or Korean glyphs.
/// Adds macOS system fonts that do, as fallbacks.
fn install_fallback_fonts(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/Library/Fonts/Arial Unicode.ttf",
        // Linux, for development.
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ];
    for (i, path) in CANDIDATES.iter().enumerate() {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let families = [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
            .into_iter()
            .map(|family| egui::epaint::text::InsertFontFamily {
                family,
                priority: egui::epaint::text::FontPriority::Lowest,
            })
            .collect();
        ctx.add_font(egui::epaint::text::FontInsert::new(
            &format!("fallback-{i}"),
            egui::FontData::from_owned(bytes),
            families,
        ));
    }
}
