//! The meeting page: record button, editable title and tags, the transcript
//! reader with the notes beside it, and actions for the file.

use eframe::egui::{self, Align, Align2, FontId, Frame, Layout, Margin, RichText, Sense, Ui, Vec2};

use super::library::{DEFAULT_TITLE, Format};
use super::{App, Page, language_name, platform, theme};
use crate::meeting;
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
        let mut notes_open = self.reader.notes_open;
        egui::Panel::right("notes_panel")
            .resizable(true)
            .default_size(300.0)
            .size_range(200.0..=640.0)
            .show_separator_line(false)
            .frame(Frame::new().inner_margin(Margin {
                left: 12,
                ..Margin::ZERO
            }))
            .show_collapsible(ui, &mut notes_open, |ui| {
                theme::card(ui)
                    .inner_margin(Margin::symmetric(14, 12))
                    .show(ui, |ui| self.notes_editor(ui));
            });
        self.reader.notes_open = notes_open;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            theme::card(ui)
                .inner_margin(Margin::symmetric(16, 12))
                .show(ui, |ui| self.transcript_view(ui));
        });
    }

    fn header(&mut self, ui: &mut Ui) {
        let recording = self.recording();
        let can_record = self.engine_ready() && !self.open.read_only();
        ui.horizontal(|ui| {
            let hint = if recording {
                "Stop recording"
            } else if self.open.read_only() {
                "Convert to Markdown to continue this meeting"
            } else {
                "Start recording"
            };
            if theme::record_button(ui, recording, recording || can_record)
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
                // Leaves room for the meters on the right.
                ui.set_max_width((ui.available_width() - 190.0).max(140.0));
                self.title_field(ui);
                ui.horizontal(|ui| self.status(ui));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.meters(ui));
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            self.tags_field(ui);
            ui.add_space(12.0);
            self.setup_line(ui);
        });
    }

    /// The title, edited in place. Old transcripts show theirs read-only.
    fn title_field(&mut self, ui: &mut Ui) {
        if self.open.read_only() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.open.meeting.title).size(18.0).strong());
                theme::pill(ui, "txt  ·  read-only", ui.visuals().weak_text_color());
            });
            return;
        }
        let edit = egui::TextEdit::singleline(&mut self.open.meeting.title)
            .id(self.open.view_id.with("title"))
            .font(FontId::proportional(18.0))
            .hint_text("Meeting title")
            .frame(Frame::NONE)
            .margin(Margin::ZERO)
            .desired_width(f32::INFINITY);
        let response = ui.add(edit).on_hover_text("Click to rename");
        if response.changed() {
            self.mark_edited();
        }
        if response.lost_focus() {
            let title = meeting::one_line(&self.open.meeting.title);
            let title = if title.is_empty() {
                DEFAULT_TITLE.to_string()
            } else {
                title
            };
            if title != self.open.meeting.title {
                self.open.meeting.title = title;
                self.mark_edited();
            }
        }
    }

    fn tags_field(&mut self, ui: &mut Ui) {
        if self.open.read_only() {
            return;
        }
        ui.weak("Tags");
        let edit = egui::TextEdit::singleline(&mut self.open.tags_text)
            .id(self.open.view_id.with("tags"))
            .hint_text("team, client")
            .desired_width(200.0);
        if ui.add(edit).on_hover_text("Separate tags with commas").changed() {
            self.open.meeting.tags = meeting::parse_tags_input(&self.open.tags_text);
            self.mark_edited();
        }
    }

    /// Status chip, plus Whisper speed once there is one.
    fn status(&mut self, ui: &mut Ui) {
        if let Some(session) = &self.session {
            let t = self.session_offset + session.started.elapsed().as_secs_f32();
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
        let m = &self.open.meeting;
        if !self.recording() && self.open.path.is_some() {
            ui.weak(format!(
                "{}  ·  {}",
                m.date.format("%a %b %-d, %H:%M"),
                meeting::format_duration(m.duration)
            ));
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
            if ui.add_enabled(self.can_download(), button).clicked() {
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

    /// Free-form notes, saved in the meeting file above the transcript.
    fn notes_editor(&mut self, ui: &mut Ui) {
        ui.set_width(ui.available_width());
        theme::card_title(ui, "Notes");
        if self.open.read_only() {
            ui.weak("Old transcripts are read-only. Convert to Markdown to add notes.");
            if ui.button("Convert to Markdown").clicked() {
                self.convert_to_markdown();
            }
            return;
        }
        let height = ui.available_height();
        let view_id = self.open.view_id;
        egui::ScrollArea::vertical()
            .id_salt(view_id.with("notes scroll"))
            .auto_shrink(false)
            .show(ui, |ui| {
                let edit = egui::TextEdit::multiline(&mut self.open.meeting.notes)
                    .id(view_id.with("notes"))
                    .hint_text("Type notes here, also while recording.\nThey are saved with the transcript.")
                    .frame(Frame::NONE)
                    .margin(Margin::ZERO)
                    .desired_width(f32::INFINITY);
                if ui.add_sized([ui.available_width(), height], edit).changed() {
                    self.mark_edited();
                }
            });
    }

    fn footer(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let has_content = !self.transcript.is_empty() || !self.open.meeting.notes.is_empty();
            ui.add_enabled_ui(has_content, |ui| {
                ui.menu_button("Copy all", |ui| {
                    if ui.button("As Markdown (title, notes, transcript)").clicked() {
                        ui.ctx().copy_text(self.export_text(Format::Markdown));
                    }
                    if ui.button("As plain text (transcript)").clicked() {
                        ui.ctx().copy_text(self.export_text(Format::Text));
                    }
                });
            });
            if let Some(path) = self.open.path.clone()
                && ui
                    .button("Show file")
                    .on_hover_text(path.display().to_string())
                    .clicked()
            {
                self.save_open();
                platform::reveal(&path);
            }
            ui.add_enabled_ui(has_content && !self.exporting(), |ui| self.export_menu(ui));
            if self.open.read_only()
                && ui
                    .button("Convert to Markdown")
                    .on_hover_text("Makes a .md meeting you can add notes to. The .txt file is kept.")
                    .clicked()
            {
                self.convert_to_markdown();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.footer_status(ui));
        });
    }

    fn export_menu(&mut self, ui: &mut Ui) {
        ui.menu_button("Export", |ui| {
            let dir = self.open_meeting_dir();
            if !self.open.read_only() && ui.button("Save .txt next to the meeting").clicked() {
                self.export_to(Format::Text, &dir);
            }
            if self.open.read_only() && ui.button("Convert to Markdown").clicked() {
                self.convert_to_markdown();
            }
            ui.separator();
            if ui.button("Export as .txt…").clicked() {
                self.pick_export_folder(Format::Text);
            }
            if ui.button("Export as .md…").clicked() {
                self.pick_export_folder(Format::Markdown);
            }
        });
    }

    /// Notices, then whether everything is saved, right-aligned.
    fn footer_status(&mut self, ui: &mut Ui) {
        let saved = if self.open.read_only() {
            "Read-only"
        } else if self.open.unsaved_changes() {
            "Saving…"
        } else if self.open.path.is_some() {
            "Saved"
        } else {
            "Not saved yet"
        };
        let hover = self.open.path.as_ref().map_or_else(
            || "Saved once it has a title, notes or lines".into(),
            |p| p.display().to_string(),
        );
        ui.weak(saved).on_hover_text(hover);
        if self.transcript.echo_removed > 0 {
            ui.weak(format!("{} echo lines removed ·", self.transcript.echo_removed));
        }
        let mut show = None;
        if let Some(notice) = self.notice() {
            if let Some(file) = &notice.file
                && ui.small_button("Show").clicked()
            {
                show = Some(file.clone());
            }
            ui.label(RichText::new(&notice.text).color(theme::accent(ui)));
        }
        if let Some(file) = show {
            platform::reveal(&file);
        }
        if self.exporting() {
            ui.spinner();
            ui.weak("Choosing a folder…");
        }
    }
}
