//! Importing a recording in the window: the Import button and dropping a
//! file on the window, the import on its own thread (Whisper's work goes
//! through the app's engine, after everything else), its progress in the
//! sidebar, and opening the meeting it made.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, anyhow};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, RichText, Ui};

use super::{App, platform, theme};
use crate::import::{self, Stage};
use crate::media;
use crate::transcriber::{DictationJob, Job, Output};

/// An import on its thread.
pub(super) struct Import {
    file: PathBuf,
    stage: Arc<Mutex<Stage>>,
    cancel: Arc<AtomicBool>,
    done: Receiver<anyhow::Result<PathBuf>>,
}

/// The file dialog for Import, open on a background thread.
pub(super) struct ImportPick(Receiver<platform::Picked>);

impl App {
    /// Asks for a recording to import.
    pub(super) fn pick_import_file(&mut self) {
        if self.import_pick.is_some() {
            return;
        }
        let start = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        let ctx = self.ctx.clone();
        let rx = platform::pick_file(
            "Choose a recording to transcribe",
            &start,
            media::EXTENSIONS,
            move || ctx.request_repaint(),
        );
        self.import_pick = Some(ImportPick(rx));
    }

    /// Starts importing `file`: read, voices told apart, transcribed, saved
    /// as a new meeting.
    pub(super) fn start_import(&mut self, file: PathBuf) {
        if let Some(busy) = &self.import {
            self.report(format!(
                "Still transcribing {}: one recording at a time.",
                file_name(&busy.file)
            ));
            return;
        }
        if !self.model.is_downloaded() {
            self.report(format!(
                "Download a Whisper model in Settings first: {} is not downloaded.",
                self.model.label
            ));
            return;
        }
        self.ensure_engine();
        let Some(engine) = &self.engine else {
            return self.report("Whisper is not running.".into());
        };
        let jobs = engine.jobs.clone();
        let language = self.language.map(str::to_string);
        let meetings_dir = self.meetings_dir_path();
        let stage = Arc::new(Mutex::new(Stage::Reading { seconds: 0.0 }));
        let cancel = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = crossbeam_channel::bounded(1);
        let ctx = self.ctx.clone();
        let (thread_file, thread_stage, thread_cancel) = (file.clone(), stage.clone(), cancel.clone());
        std::thread::Builder::new()
            .name("import".into())
            .spawn(move || {
                let result = run(&thread_file, jobs, language, &meetings_dir, thread_cancel, |s| {
                    *thread_stage.lock().unwrap_or_else(|e| e.into_inner()) = s;
                    ctx.request_repaint();
                });
                let _ = done_tx.send(result);
                ctx.request_repaint();
            })
            .expect("failed to start the import thread");
        self.import = Some(Import {
            file,
            stage,
            cancel,
            done,
        });
    }

    /// Files dropped on the window: recordings are imported.
    pub(super) fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        for path in dropped {
            if media::is_media(&path) {
                self.start_import(path);
            } else {
                self.report(format!(
                    "{} is not a recording this app can transcribe (it reads {}).",
                    file_name(&path),
                    media::EXTENSIONS.join(", ")
                ));
            }
        }
    }

    pub(super) fn poll_import(&mut self) {
        if let Some(ImportPick(rx)) = &self.import_pick
            && let Ok(picked) = rx.try_recv()
        {
            self.import_pick = None;
            match picked {
                platform::Picked::Path(file) => self.start_import(file),
                platform::Picked::Cancelled => {}
                platform::Picked::NoDialog => self.report(
                    "There is no file dialog here (install zenity or kdialog): drop the recording on the window instead."
                        .into(),
                ),
            }
        }

        let Some(import) = &self.import else { return };
        let Ok(result) = import.done.try_recv() else { return };
        let (file, cancelled) = (import.file.clone(), import.cancel.load(Ordering::Relaxed));
        self.import = None;
        match result {
            Ok(path) => {
                self.list.scanned = None;
                // Not while another meeting is being recorded into.
                if self.recording() {
                    self.notify(format!("Transcribed {}", file_name(&file)), Some(path));
                } else {
                    self.open_meeting(&path, crate::meetings::Kind::Markdown);
                }
            }
            Err(_) if cancelled => {}
            Err(e) => self.report(format!("Could not transcribe {}: {e:#}", file_name(&file))),
        }
    }

    /// Import in the sidebar: the button, or how far the import is.
    pub(super) fn import_card(&mut self, ui: &mut Ui) {
        let Some(import) = &self.import else {
            let button = egui::Button::new("⬆  Import recording…")
                .min_size(egui::Vec2::new(ui.available_width(), 26.0))
                .corner_radius(8);
            if ui
                .add_enabled(self.import_pick.is_none(), button)
                .on_hover_text(
                    "Transcribe a recording (a Teams or Zoom video, MP4, M4A, MP3, WAV…), its speakers told \
                     apart. You can also drop the file on the window.",
                )
                .clicked()
            {
                self.pick_import_file();
            }
            return;
        };
        let stage = *import.stage.lock().unwrap_or_else(|e| e.into_inner());
        let mut cancel = false;
        theme::card(ui).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(file_name(&import.file)).strong());
            ui.weak(import::stage_text(&stage));
            let fraction = match stage {
                Stage::DownloadingModels { done, total } => Some(done as f32 / total.max(1) as f32),
                Stage::Transcribing {
                    done_seconds,
                    total_seconds,
                } => Some(done_seconds / total_seconds.max(0.001)),
                Stage::Reading { .. } | Stage::FindingSpeakers => None,
            };
            let bar = egui::ProgressBar::new(fraction.unwrap_or(0.0)).desired_height(6.0);
            ui.add(if fraction.is_none() { bar.animate(true) } else { bar });
            let stopping = import.cancel.load(Ordering::Relaxed);
            if ui.add_enabled(!stopping, egui::Button::new("Cancel").small()).clicked() {
                cancel = true;
            }
        });
        if cancel {
            import.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The import thread: the recording becomes a meeting file.
fn run(
    file: &Path,
    jobs: Sender<Job>,
    language: Option<String>,
    meetings_dir: &Path,
    cancel: Arc<AtomicBool>,
    stage: impl FnMut(Stage),
) -> anyhow::Result<PathBuf> {
    let (lines, duration) = import::transcribe_file(file, true, cancel, stage, |samples| {
        let (reply, answer) = crossbeam_channel::bounded(1);
        jobs.send(Job::Background(DictationJob {
            samples: samples.to_vec(),
            language: language.clone(),
            prompt: None,
            reply,
        }))
        .map_err(|_| anyhow!("Whisper has stopped"))?;
        match answer.recv().context("Whisper has stopped")? {
            Output::Dictation(Ok(recognized)) => Ok(recognized),
            Output::Dictation(Err(e)) => Err(anyhow!(e)),
            other => Err(anyhow!("unexpected answer from Whisper: {other:?}")),
        }
    })?;
    if lines.is_empty() {
        anyhow::bail!("no speech was found in it");
    }
    import::save_new(meetings_dir, &import::meeting_for(file, lines, duration))
}
