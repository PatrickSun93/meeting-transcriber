//! The meeting page: record button and status, the live transcript, and
//! actions for the saved file.

use eframe::egui::{self, Align, Align2, FontId, Frame, Layout, Margin, RichText, Sense, Ui, Vec2};

use super::{App, Page, language_name, platform, theme};
use crate::transcript::{Speaker, format_time};

impl App {
    pub(super) fn meeting_page(&mut self, ui: &mut Ui) {
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            self.header(ui);
        });
        ui.add_space(12.0);
        egui::Panel::bottom("meeting_footer")
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().inner_margin(Margin {
                top: 10,
                ..Margin::ZERO
            }))
            .show(ui, |ui| self.footer(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| {
            theme::card(ui)
                .inner_margin(Margin::symmetric(16, 12))
                .show(ui, |ui| self.transcript_view(ui));
        });
    }

    fn header(&mut self, ui: &mut Ui) {
        let recording = self.recording();
        let ready = self.engine_ready();
        ui.horizontal(|ui| {
            let hint = if recording { "Stop recording" } else { "Start recording" };
            if theme::record_button(ui, recording, recording || ready)
                .on_hover_text(hint)
                .clicked()
            {
                if recording {
                    self.stop_session();
                } else {
                    self.start_session();
                }
            }
            ui.add_space(10.0);
            ui.vertical(|ui| {
                let title = self
                    .save_path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map_or_else(|| "New meeting".to_string(), |s| s.to_string_lossy().into_owned());
                ui.label(RichText::new(title).size(18.0).strong());
                ui.horizontal(|ui| self.status(ui));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.meters(ui));
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| self.setup_line(ui));
    }

    /// Status chip, plus Whisper speed once there is one.
    fn status(&mut self, ui: &mut Ui) {
        if let Some(session) = &self.session {
            let t = session.started.elapsed().as_secs_f32();
            theme::pill(ui, &format!("●  Recording  {}", format_time(t)), theme::RECORD_RED);
            ui.weak(session.source_names.join(" + "));
        } else if self.download.as_ref().is_some_and(|d| d.model == self.model) {
            theme::pill(ui, "Downloading model…", theme::accent(ui));
        } else if !self.model.is_downloaded() {
            theme::pill(ui, "Model not downloaded", ui.visuals().warn_fg_color);
        } else if self.engine.is_none() {
            theme::pill(ui, "Model not loaded", ui.visuals().warn_fg_color);
            if ui.small_button("Load model").clicked() {
                self.ensure_engine();
            }
        } else if !self.engine_ready() {
            ui.spinner();
            theme::pill(ui, "Loading model…", theme::accent(ui));
        } else {
            theme::pill(ui, "●  Ready", theme::READY_GREEN);
        }
        if let Some(speed) = self.speed {
            ui.weak(format!("Whisper {speed:.2}× real time"))
                .on_hover_text("Seconds of processing per second of speech. Below 1 keeps up live.");
        }
    }

    /// Input levels for Me and Others; flat when not recording.
    fn meters(&self, ui: &mut Ui) {
        // A fixed-width column: called from a right-to-left layout, a plain
        // vertical would take the whole row and hug the left.
        let size = Vec2::new(170.0, 2.0 * ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(size, Layout::top_down(Align::Min), |ui| {
            for speaker in [Speaker::Me, Speaker::Others] {
                let on = match speaker {
                    Speaker::Me => self.capture_me,
                    Speaker::Others => self.capture_others,
                };
                let level = self.session.as_ref().map_or(0.0, |s| s.levels.get(speaker) * 8.0);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(52.0, 16.0), Sense::hover());
                    let color = if on {
                        theme::speaker_color(speaker)
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    ui.painter().text(
                        rect.left_center(),
                        Align2::LEFT_CENTER,
                        speaker.label(),
                        FontId::proportional(12.5),
                        color,
                    );
                    if on {
                        theme::level_meter(ui, level, color, 110.0);
                    } else {
                        ui.weak("off");
                    }
                });
            }
        });
    }

    /// What will be used for the next recording, or what is missing.
    fn setup_line(&mut self, ui: &mut Ui) {
        if self.download.as_ref().is_some_and(|d| d.model == self.model) {
            ui.label(format!("Downloading {}", self.model.label));
            self.download_progress(ui);
            return;
        }
        if !self.model.is_downloaded() {
            ui.label(format!("{} is not downloaded yet.", self.model.label));
            let button = egui::Button::new(format!("⬇  Download ({} MB)", self.model.size_mb));
            if ui.add_enabled(self.download.is_none(), button).clicked() {
                self.start_download(self.model);
            }
            return;
        }
        let sources = match (self.capture_me, self.capture_others) {
            (true, true) => "Me + Others",
            (true, false) => "Me only",
            (false, true) => "Others only",
            (false, false) => "No sources on",
        };
        ui.weak(format!(
            "{}  ·  {}  ·  {}",
            self.model.label,
            language_name(self.language),
            sources
        ));
        if ui.link("Change in Settings").clicked() {
            self.page = Page::Settings;
        }
    }

    fn transcript_view(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.transcript.is_empty()
                    && self.transcript.preview(Speaker::Me).is_none()
                    && self.transcript.preview(Speaker::Others).is_none()
                {
                    empty_state(ui);
                }
                for line in self.transcript.lines() {
                    ui.horizontal_top(|ui| {
                        timestamp(ui, line.start);
                        theme::speaker_pill(ui, line.speaker, false);
                        ui.add(egui::Label::new(&line.text).wrap().selectable(true));
                    });
                    ui.add_space(2.0);
                }
                for speaker in [Speaker::Me, Speaker::Others] {
                    if let Some(p) = self.transcript.preview(speaker) {
                        ui.horizontal_top(|ui| {
                            timestamp(ui, p.start);
                            theme::speaker_pill(ui, speaker, true);
                            ui.add(egui::Label::new(RichText::new(&p.text).italics().weak()).wrap());
                        });
                    }
                }
            });
    }

    fn footer(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let has_lines = !self.transcript.is_empty();
            if ui.add_enabled(has_lines, egui::Button::new("Copy all")).clicked() {
                ui.ctx().copy_text(self.transcript.to_text());
            }
            if let Some(path) = self.save_path.clone()
                && ui
                    .button("Show file")
                    .on_hover_text(path.display().to_string())
                    .clicked()
            {
                platform::reveal_in_finder(&path);
            }
            if ui
                .add_enabled(has_lines && !self.recording(), egui::Button::new("New transcript"))
                .on_hover_text("Clears the window. The saved file is kept.")
                .clicked()
            {
                self.transcript.clear();
                self.save_path = None;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(path) = &self.save_path {
                    ui.weak(format!(
                        "Saving to {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
                if self.transcript.echo_removed > 0 {
                    ui.weak(format!("{} echo lines removed ·", self.transcript.echo_removed));
                }
            });
        });
    }
}

/// A small fixed-width monospace time, so rows line up.
fn timestamp(ui: &mut Ui, seconds: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(56.0, 20.0), Sense::hover());
    ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        format_time(seconds),
        FontId::monospace(12.0),
        ui.visuals().weak_text_color(),
    );
}

fn empty_state(ui: &mut Ui) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("No transcript yet").size(17.0).strong().weak());
        ui.add_space(4.0);
        ui.weak("Press the red button, then join or continue your meeting.");
        ui.weak("Your microphone shows as Me; everything the Mac plays shows as Others.");
    });
}
