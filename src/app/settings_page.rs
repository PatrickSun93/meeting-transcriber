//! The settings page: models, storage folders, recognition options, about.

use eframe::egui::{self, Align, Layout, RichText, Ui};

use super::{App, Folder, LANGUAGES, format_size, language_name, platform, theme};
use crate::models::{self, MODELS, ModelInfo};
use crate::settings;

impl App {
    pub(super) fn settings_page(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .id_salt("settings_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.heading("Settings");
                ui.add_space(10.0);
                for section in [
                    Self::model_card,
                    Self::storage_card,
                    Self::recognition_card,
                    Self::about_card,
                ] {
                    theme::card(ui).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        section(self, ui);
                    });
                    ui.add_space(12.0);
                }
            });
    }

    fn model_card(&mut self, ui: &mut Ui) {
        theme::card_title(ui, "Model");
        ui.weak("Whisper runs on this Mac. Larger models are more accurate but slower.");
        if self.recording() {
            ui.colored_label(ui.visuals().warn_fg_color, "Stop recording to switch models.");
        }
        ui.add_space(6.0);
        for (i, model) in MODELS.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            self.model_row(ui, *model);
        }
        ui.add_space(6.0);
        let used: u64 = MODELS.iter().filter_map(ModelInfo::file_size).sum();
        ui.weak(format!("Downloaded models use {} on disk.", format_size(used)));
    }

    fn model_row(&mut self, ui: &mut Ui, model: ModelInfo) {
        let in_use = model == self.model;
        let size = model.file_size();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(model.label).strong());
                    if in_use {
                        theme::pill(ui, "In use", theme::accent(ui));
                    }
                });
                ui.weak(match size {
                    Some(bytes) => format!("{}  ·  Downloaded", format_size(bytes)),
                    None => format!("{} MB  ·  Not downloaded", model.size_mb),
                });
            });
            // Right to left: the last button added is the leftmost.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.model_actions(ui, model, size.is_some())
            });
        });
    }

    fn model_actions(&mut self, ui: &mut Ui, model: ModelInfo, downloaded: bool) {
        let in_use = model == self.model;
        let moving = self.model_move.is_some();
        if self.download.as_ref().is_some_and(|d| d.model == model) {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| self.download_progress(ui));
        } else if self.confirm_delete == Some(model) {
            let delete = egui::Button::new(RichText::new("Delete").color(egui::Color32::WHITE)).fill(theme::RECORD_RED);
            if ui.add(delete).clicked() {
                self.delete_model(model);
            }
            if ui.button("Cancel").clicked() {
                self.confirm_delete = None;
            }
            ui.label(format!(
                "Delete {}?",
                model.file_size().map_or_else(String::new, format_size)
            ));
        } else if downloaded {
            let can_delete = !moving && !(in_use && self.recording());
            if ui.add_enabled(can_delete, egui::Button::new("🗑  Delete")).clicked() {
                self.confirm_delete = Some(model);
            }
            if ui.button("Show in Finder").clicked() {
                platform::reveal_in_finder(&model.path());
            }
            if !in_use {
                let can_use = !moving && !self.recording();
                if ui.add_enabled(can_use, egui::Button::new("Use")).clicked() {
                    self.use_model(model);
                }
            }
        } else {
            let button = egui::Button::new(format!("⬇  Download ({} MB)", model.size_mb));
            if ui.add_enabled(self.can_download(), button).clicked() {
                self.start_download(model);
            }
        }
    }

    fn storage_card(&mut self, ui: &mut Ui) {
        theme::card_title(ui, "Storage");
        self.folder_row(ui, Folder::Models);
        ui.separator();
        self.folder_row(ui, Folder::Meetings);
    }

    fn folder_row(&mut self, ui: &mut Ui, folder: Folder) {
        let (title, hint, setting) = match folder {
            Folder::Models => (
                "Models folder",
                "Downloaded Whisper models are kept here.",
                &self.models_dir,
            ),
            Folder::Meetings => (
                "Meetings folder",
                "Meetings are saved here as Markdown and listed in the sidebar. Existing files stay where they are.",
                &self.meetings_dir,
            ),
        };
        let is_default = setting.is_empty();
        let env_dir = (folder == Folder::Models).then(models::env_models_dir).flatten();
        ui.label(RichText::new(title).strong());
        ui.weak(hint);
        let shown = env_dir.clone().unwrap_or_else(|| self.folder_path(folder));
        ui.add(
            egui::Label::new(RichText::new(shown.display().to_string()).monospace())
                .wrap()
                .selectable(true),
        );
        if let Some(dir) = &env_dir {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "{} is set, so models are read from {} and this setting is ignored.",
                    models::MODELS_ENV,
                    dir.display()
                ),
            );
        }

        if folder == Folder::Models && self.models_folder_prompts(ui) {
            return;
        }

        let busy = self.folder_pick.is_some()
            || (folder == Folder::Models
                && (env_dir.is_some() || self.recording() || self.download.is_some() || self.model_move.is_some()));
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Change…")).clicked() {
                self.pick_folder(folder);
            }
            if ui.button("Show in Finder").clicked() {
                platform::open_folder(&shown);
            }
            if ui
                .add_enabled(!busy && !is_default, egui::Button::new("Reset to default"))
                .clicked()
            {
                self.set_folder(folder, String::new());
            }
            if self.folder_pick.as_ref().is_some_and(|p| p.folder == folder) {
                ui.spinner();
                ui.weak("Choosing a folder…");
            }
        });
        // Says why Change… is greyed out (the env var case is explained above).
        if folder == Folder::Models && env_dir.is_none() && (self.recording() || self.download.is_some()) {
            ui.weak("Finish recording or downloading to change the folder.");
        }
    }

    /// The "move models?" question and the move's progress. Returns true
    /// while either is showing, in place of the folder buttons.
    fn models_folder_prompts(&mut self, ui: &mut Ui) -> bool {
        if let Some(m) = &self.model_move {
            let (done, total) = *m.progress.lock().unwrap();
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Moving model {} of {total}…", (done + 1).min(total)));
            });
            return true;
        }
        let Some(offer) = &self.move_offer else { return false };
        let question = format!(
            "Move {} model{} ({}) to the new folder?",
            offer.count,
            if offer.count == 1 { "" } else { "s" },
            format_size(offer.bytes)
        );
        let to = offer.to.display().to_string();
        // A download writes into the current folder; switching now would strand it.
        let downloading = self.download.is_some();
        egui::Frame::new()
            .fill(theme::accent(ui).gamma_multiply(0.1))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(question).strong());
                ui.weak(format!("New folder: {to}"));
                if downloading {
                    ui.weak("Waiting for the download to finish…");
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!downloading, egui::Button::new("Move")).clicked()
                        && let Some(offer) = self.move_offer.take()
                    {
                        self.start_model_move(offer);
                    }
                    if ui
                        .add_enabled(!downloading, egui::Button::new("Leave them"))
                        .on_hover_text("Switch folders; models you need again can be downloaded.")
                        .clicked()
                        && let Some(offer) = self.move_offer.take()
                    {
                        self.apply_models_dir(offer.setting);
                    }
                    if ui.button("Cancel").on_hover_text("Keep the current folder.").clicked() {
                        self.move_offer = None;
                    }
                });
            });
        true
    }

    fn recognition_card(&mut self, ui: &mut Ui) {
        theme::card_title(ui, "Recognition & audio");
        let recording = self.recording();
        ui.add_enabled_ui(!recording, |ui| {
            ui.horizontal(|ui| {
                ui.label("Default language");
                egui::ComboBox::from_id_salt("language")
                    .selected_text(language_name(self.language))
                    .show_ui(ui, |ui| {
                        for (code, name) in LANGUAGES {
                            ui.selectable_value(&mut self.language, *code, *name);
                        }
                    });
            });
            ui.weak("Auto-detect handles mixed languages. Picking one makes short phrases more reliable.");
            ui.add_space(6.0);
            ui.label("Capture");
            ui.checkbox(&mut self.capture_me, "Me (microphone)");
            ui.checkbox(&mut self.capture_others, "Others (Mac audio)");
        });
        if recording {
            ui.weak("Language and sources apply from the next recording.");
        }
        ui.add_space(6.0);
        ui.checkbox(&mut self.transcript.echo.enabled, "Remove speaker echo");
        ui.weak(
            "When the meeting plays through speakers, your mic hears the others too. \
             This drops your lines that repeat what Others just said. Not needed with headphones.",
        );
    }

    fn about_card(&mut self, ui: &mut Ui) {
        theme::card_title(ui, "About");
        let settings_file = settings::settings_path();
        egui::Grid::new("about")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.weak("Version");
                ui.label(env!("CARGO_PKG_VERSION"));
                ui.end_row();
                ui.weak("Models folder");
                ui.add(egui::Label::new(RichText::new(models::models_dir().display().to_string()).monospace()).wrap());
                ui.end_row();
                ui.weak("Settings file");
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(settings_file.display().to_string()).monospace()).wrap());
                    if settings_file.exists() && ui.small_button("Show").clicked() {
                        platform::reveal_in_finder(&settings_file);
                    }
                });
                ui.end_row();
            });
    }
}
