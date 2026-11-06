//! A screen recording in the window: started with Record on the screenshot
//! overlay, recorded on a thread of its own (`capture::record`), with a
//! small window beside the area (the time so far, and Stop) and a thin
//! outline around it, both left out of the video. Stop there, a capture
//! hotkey, a Screenshot button or the menu bar (or tray) item stops it. The video goes beside the open meeting, with a line in its
//! notes, or into the Recordings folder; once it is done, a banner offers
//! to open it, and to transcribe it when it has sound.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, RichText, Ui, Vec2, ViewportBuilder, ViewportId, vec2};

use super::capture::When;
use super::dictation_page::banner;
use super::overlay::{PLACE_FRAMES, beside, keep_at, outline};
use super::{App, platform, theme};
use crate::capture::files::Kind;
use crate::capture::record::{self, Recorded, timer};
use crate::capture::scrolling::Area;
use crate::capture::{hide_from_captures, raise_overlay};
use crate::media;

/// The control window's title, by which the system's window is found; the
/// outline's bars add a number to it.
const TITLE: &str = "Meeting Transcriber recording";
/// The control window's size, in points.
const SIZE: Vec2 = vec2(250.0, 40.0);
/// Between the area and the control window, in points.
const GAP: f32 = 10.0;
/// For the overlay to leave the screen before the first frame.
const START_WAIT: Duration = Duration::from_millis(250);
/// How often the thread looks whether it is to stop.
const POLL: Duration = Duration::from_millis(100);
/// How long quitting the app waits for the video to be finished.
const EXIT_WAIT: Duration = Duration::from_secs(10);
const RED: Color32 = Color32::from_rgb(220, 38, 38);

/// News from the recording's thread.
enum Update {
    Started,
    /// Finished, and whether the file has sound.
    Done(Result<(Recorded, bool)>),
}

/// A screen recording from Record on the overlay until its file is done.
pub(super) struct Recording {
    area: Area,
    path: PathBuf,
    /// The window was hidden for the screenshot, and comes back once done.
    show_window_after: bool,
    stop: Arc<AtomicBool>,
    updates: Receiver<Update>,
    /// When it started recording, once it has.
    started: Option<Instant>,
    frames: u32,
    /// Whether each window is raised and out of the video yet: the control
    /// window, then the outline's four bars.
    ready: [bool; 5],
}

/// A finished recording, offered in a banner until it is dismissed.
pub(super) struct Finished {
    path: PathBuf,
    seconds: f64,
    sound: bool,
}

impl App {
    /// Starts recording `area`, on a thread, into the open meeting's folder
    /// (with a line in its notes) or the Recordings folder. With
    /// `show_window_after`, the window (hidden for the screenshot) comes
    /// back once it is done.
    pub(super) fn start_recording(&mut self, area: Area, when: When, show_window_after: bool) {
        let (path, note) = match self.capture_place(when, Kind::Recording) {
            Ok(place) => place,
            Err(e) => {
                self.report(format!("Could not record the screen: {e:#}"));
                self.show_window(&self.ctx.clone());
                return;
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = crossbeam_channel::unbounded();
        let (ctx, flag, file) = (self.ctx.clone(), stop.clone(), path.clone());
        let options = self.capture.settings.record_options();
        let spawned = std::thread::Builder::new()
            .name("screen recording".into())
            .spawn(move || {
                std::thread::sleep(START_WAIT);
                let result = record::start(&area, &file, options).and_then(|recorder| {
                    let _ = tx.send(Update::Started);
                    ctx.request_repaint();
                    while !flag.load(Ordering::Relaxed) && recorder.failure().is_none() {
                        std::thread::sleep(POLL);
                    }
                    let recorded = recorder.stop()?;
                    Ok((recorded, media::has_sound(&file)))
                });
                let _ = tx.send(Update::Done(result));
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            self.report(format!("Could not record the screen: {e}"));
            self.show_window(&self.ctx.clone());
            return;
        }
        self.capture.pending.push(path.clone());
        if let Some(line) = note {
            self.link_in_notes(&path, line);
        }
        self.capture.recording = Some(Recording {
            area,
            path,
            show_window_after,
            stop,
            updates: rx,
            started: None,
            frames: 0,
            ready: [false; 5],
        });
    }

    /// Asks the recording to stop, if one is going; its file is finished on
    /// its thread.
    pub(super) fn stop_recording(&mut self) {
        if let Some(recording) = &self.capture.recording {
            recording.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Whether the screen is being recorded (or its file finished).
    pub(super) fn recording_screen(&self) -> bool {
        self.capture.recording.is_some()
    }

    /// Whether `path` is the file the screen is being recorded into.
    pub(super) fn recording_to(&self, path: &Path) -> bool {
        self.capture.recording.as_ref().is_some_and(|r| r.path == path)
    }

    /// Takes in the thread's news, and the file once it is done.
    pub(super) fn recording_logic(&mut self, ctx: &egui::Context) {
        let Some(recording) = &mut self.capture.recording else {
            return;
        };
        let mut done = None;
        for update in recording.updates.try_iter() {
            match update {
                Update::Started => recording.started = Some(Instant::now()),
                Update::Done(result) => done = Some(result),
            }
        }
        if let Some(result) = done {
            self.recording_done(ctx, result);
        }
    }

    fn recording_done(&mut self, ctx: &egui::Context, result: Result<(Recorded, bool)>) {
        let Some(recording) = self.capture.recording.take() else {
            return;
        };
        self.capture.pending.retain(|p| *p != recording.path);
        // A failed recording may still have kept what came before it;
        // without that, its line in the notes leads nowhere.
        let kept = std::fs::metadata(&recording.path).is_ok_and(|m| m.len() > 0);
        self.file_written(&recording.path, result.is_err() && !kept);
        let name = file_name(&recording.path);
        match result {
            Ok((recorded, sound)) => {
                let length = timer(recorded.seconds.round() as u64);
                self.notify(format!("Saved {name} ({length})"), Some(recording.path.clone()));
                self.capture.recorded = Some(Finished {
                    path: recording.path,
                    seconds: recorded.seconds,
                    sound,
                });
                if recording.show_window_after {
                    self.show_window(ctx);
                }
            }
            Err(e) => {
                self.report(format!("The screen recording {name} failed: {e:#}"));
                self.show_window(ctx);
            }
        }
    }

    /// Stops the recording and waits (a little) for its file, when the app quits.
    pub(super) fn finish_recording(&mut self) {
        let Some(recording) = &self.capture.recording else {
            return;
        };
        recording.stop.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + EXIT_WAIT;
        let mut done = None;
        while let Ok(update) = recording.updates.recv_deadline(deadline) {
            if let Update::Done(result) = update {
                done = Some(result);
                break;
            }
        }
        if let Some(result) = done {
            self.recording_done(&self.ctx.clone(), result);
        }
    }

    /// The control window and the outline, while recording. Called from
    /// the window's frame.
    pub(super) fn recording_windows(&mut self, ctx: &egui::Context) {
        let Some(recording) = &mut self.capture.recording else {
            return;
        };
        recording.frames += 1;
        let placing = recording.frames <= PLACE_FRAMES;
        let area = recording.area;
        let scale = area.display.scale.max(0.25);
        let (window, bars) = recording.ready.split_at_mut(1);
        outline(ctx, &area, TITLE, bars, placing);

        let origin = beside(&area, SIZE * scale, GAP * scale);
        let builder = ViewportBuilder::default()
            .with_title(TITLE)
            .with_position((origin.to_vec2() / scale).to_pos2())
            .with_inner_size(SIZE)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_active(false);
        let (started, stopping) = (recording.started, recording.stop.load(Ordering::Relaxed));
        let ready = &mut window[0];
        let stop = ctx.show_viewport_immediate(ViewportId::from_hash_of(TITLE), builder, |ui, _| {
            if !*ready {
                *ready = raise_overlay(TITLE, false) && hide_from_captures(TITLE);
            }
            if placing {
                keep_at(ui.ctx(), origin, None, scale);
            }
            control(ui, started, stopping)
        });
        if stop {
            recording.stop.store(true, Ordering::Relaxed);
        }
    }

    /// The banner about a finished recording: open it, show it, transcribe
    /// it (when it has sound), or dismiss it.
    pub(super) fn recording_banner(&mut self, ui: &mut Ui) {
        let Some(done) = &self.capture.recorded else {
            return;
        };
        let text = format!(
            "Screen recording saved: {} ({}).",
            file_name(&done.path),
            timer(done.seconds.round() as u64)
        );
        let (mut open, mut show, mut transcribe, mut dismiss) = (false, false, false, false);
        banner(ui, theme::accent(ui), &text, |ui| {
            open = ui.small_button("Open").clicked();
            show = ui.small_button(platform::SHOW_IN_FILES).clicked();
            if done.sound {
                transcribe = ui
                    .small_button("Transcribe")
                    .on_hover_text("Make a meeting of what is said in it, its speakers told apart")
                    .clicked();
            }
            dismiss = ui.small_button("Dismiss").clicked();
        });
        let path = done.path.clone();
        if open {
            platform::open_file(&path);
        }
        if show {
            platform::reveal(&path);
        }
        if transcribe {
            self.capture.recorded = None;
            self.start_import(path);
        } else if dismiss {
            self.capture.recorded = None;
        }
    }

    /// The screen recording part of the Screenshots card in Settings.
    pub(super) fn recording_settings(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("Screen recording").strong());
        ui.weak(
            "Record on the screenshot toolbar (or the screen recording hotkey) records the area as a video \
             (MP4), until you press Stop beside it, a capture hotkey, or Stop screen recording in the menu.",
        );
        if let Some(why) = record::unsupported() {
            ui.colored_label(ui.visuals().warn_fg_color, why);
            return;
        }
        let settings = &mut self.capture.settings;
        ui.checkbox(&mut settings.record_computer_sound, "With the computer's sound");
        ui.checkbox(&mut settings.record_microphone, "With the microphone");
        if !settings.sound().any() {
            ui.weak("Videos are made without sound, so they can't be transcribed.");
        }
        ui.checkbox(&mut settings.record_cursor, "With the pointer");
        ui.horizontal(|ui| {
            ui.label("Frames a second");
            egui::ComboBox::from_id_salt("record_fps")
                .selected_text(settings.record_fps.to_string())
                .show_ui(ui, |ui| {
                    for fps in record::FPS_CHOICES {
                        ui.selectable_value(&mut settings.record_fps, fps, fps.to_string());
                    }
                })
                .response
                .on_hover_text("More is smoother and makes larger files; a slow computer may not keep up with 60.");
        });
        // The Mac's recording output offers no bitrate: it picks its own.
        let mac = cfg!(target_os = "macos");
        ui.horizontal(|ui| {
            ui.label("Quality");
            ui.add_enabled_ui(!mac, |ui| {
                super::capture_settings::choice_combo(ui, "record_quality", &mut settings.record_quality)
                    .on_hover_text("How many bits a second the video gets: Low is half of Normal, High twice.")
                    .on_disabled_hover_text(
                        "On the Mac the system's encoder picks the video's bitrate itself, and offers no setting \
                         for it.",
                    );
            });
        });
        if mac {
            ui.weak("The Mac's encoder picks the quality itself.");
        }
    }
}

/// The control window's content; true when Stop is pressed.
fn control(ui: &mut Ui, started: Option<Instant>, stopping: bool) -> bool {
    let mut stop = false;
    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            match (started, stopping) {
                (_, true) => {
                    ui.spinner();
                    ui.label("Finishing the video…");
                }
                (None, false) => {
                    ui.spinner();
                    ui.label("Starting…");
                }
                (Some(at), false) => {
                    ui.label(RichText::new("●").color(RED));
                    ui.label(RichText::new(timer(at.elapsed().as_secs())).strong().monospace());
                    ui.weak("Recording");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                stop = ui
                    .add_enabled(!stopping, egui::Button::new("Stop"))
                    .on_hover_text("Stop and save the video (or press a capture hotkey)")
                    .clicked();
            });
        });
    });
    stop
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}
