//! The settings page: models, storage folders, recognition options,
//! dictation (in `dictation_page.rs`), screenshots (in `capture.rs`), about.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

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
                    Self::dictation_card,
                    Self::screenshot_card,
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

    /// With an NVIDIA card and no CUDA support yet: offers to download it,
    /// shows the download, then offers to restart into it.
    fn cuda_addon_row(&mut self, ui: &mut Ui) {
        use crate::cuda_addon::{self, State};

        if cfg!(target_os = "macos") || !self.use_gpu || crate::transcriber::GPU == Some("CUDA") {
            return;
        }
        if let Some(d) = &self.cuda_addon.download
            && let Ok(result) = d.done.try_recv()
        {
            self.cuda_addon.download = None;
            match result {
                Ok(_) => {
                    self.cuda_addon.installed = true;
                    self.cuda_addon.state = None;
                }
                Err(e) => self.errors.push(format!("Could not get CUDA support: {e:#}")),
            }
        }
        if let Some(d) = &self.cuda_addon.download {
            let (done, total) = *d.progress.lock().unwrap_or_else(|e| e.into_inner());
            ui.horizontal(|ui| {
                let fraction = total.map_or(0.0, |t| done as f32 / t.max(1) as f32);
                let text = match total {
                    Some(t) => format!("Downloading CUDA support…  {} of {}", format_size(done), format_size(t)),
                    None => format!("Downloading CUDA support…  {}", format_size(done)),
                };
                ui.add(egui::ProgressBar::new(fraction).text(text).desired_width(320.0));
                if ui.button("Cancel").clicked() {
                    d.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            });
            return;
        }
        if self.cuda_addon.installed {
            let recording = self.recording();
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    theme::READY_GREEN,
                    "CUDA support is installed. It is used once the app opens again.",
                );
                if ui
                    .add_enabled(!recording, egui::Button::new("Restart now"))
                    .on_disabled_hover_text("Stop recording first.")
                    .clicked()
                {
                    self.restart();
                }
            });
            return;
        }
        let state = self.cuda_addon.state.get_or_insert_with(cuda_addon::state).clone();
        if !state.offer() {
            return;
        }
        let text = match state {
            State::Outdated(_) => format!(
                "The CUDA support here is for another version of the app. Download the one for this version \
                 (about {} MB) to use the NVIDIA card again.",
                cuda_addon::SIZE_MB
            ),
            _ => format!(
                "An NVIDIA graphics card is here. CUDA support (about {} MB to download) makes Whisper faster on it \
                 than {}.",
                cuda_addon::SIZE_MB,
                if crate::transcriber::GPU.is_some() {
                    "Vulkan"
                } else {
                    "the processor"
                }
            ),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(text);
            if ui.button("Download CUDA support").clicked() {
                self.start_cuda_download();
            }
        });
    }

    fn start_cuda_download(&mut self) {
        let progress = Arc::new(Mutex::new((0, None)));
        let cancel = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let ctx = self.ctx.clone();
        let (p, c) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let result = crate::cuda_addon::install(None, c, |done, total| {
                *p.lock().unwrap_or_else(|e| e.into_inner()) = (done, total);
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            });
            let _ = done_tx.send(result);
            ctx.request_repaint();
        });
        self.cuda_addon.download = Some(super::CudaDownload {
            progress,
            cancel,
            done: done_rx,
        });
    }

    fn model_card(&mut self, ui: &mut Ui) {
        theme::card_title(ui, "Model");
        ui.weak(format!(
            "Whisper runs on {}. Larger models are more accurate but slower.",
            platform::THIS_COMPUTER
        ));
        match crate::transcriber::GPU {
            Some(gpu) => {
                ui.weak(format!("Whisper runs on the graphics card ({gpu})."));
            }
            None => {
                ui.weak(crate::gpu::why_cpu().unwrap_or(
                    "This build runs Whisper on the processor; a build with --features vulkan or cuda uses the \
                     graphics card.",
                ));
                ui.weak("On the processor, Small or Base keep up live more easily.");
            }
        }
        if !cfg!(target_os = "macos") {
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.use_gpu, "Use the graphics card when there is one")
                    .on_hover_text("CUDA on NVIDIA cards, Vulkan on others. Takes effect when the app is next opened.");
                if crate::gpu::has_failures()
                    && ui
                        .small_button("Try it again")
                        .on_hover_text("A graphics-card build did not start before. Try it at the next opening.")
                        .clicked()
                {
                    crate::gpu::forget_failures();
                }
            });
        }
        self.cuda_addon_row(ui);
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
            if ui.button(platform::SHOW_IN_FILES).clicked() {
                platform::reveal(&model.path());
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

    pub(super) fn folder_row(&mut self, ui: &mut Ui, folder: Folder) {
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
            Folder::Recordings => (
                "Recordings folder",
                "Dictations are saved here as WAV files, each with its text, and listed in the sidebar.",
                &self.dict.settings.recordings_dir,
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
            if ui.button(platform::SHOW_IN_FILES).clicked() {
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
        if let Some(from) = settings::expand_dir(&self.models_move_from) {
            self.unfinished_move_prompt(ui, &from);
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

    /// A move that failed part-way: its models are still used from the old
    /// folder until it is retried or given up.
    fn unfinished_move_prompt(&mut self, ui: &mut Ui, from: &std::path::Path) {
        let left = models::downloaded_in(from).len();
        egui::Frame::new()
            .fill(ui.visuals().warn_fg_color.gamma_multiply(0.1))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(format!(
                        "{left} model{} still in the old folder",
                        if left == 1 { " is" } else { "s are" }
                    ))
                    .strong(),
                );
                ui.weak(format!("Old folder: {}", from.display()));
                ui.horizontal(|ui| {
                    if ui.button("Retry move").clicked() {
                        self.spawn_model_move(from.to_path_buf());
                    }
                    if ui
                        .button("Leave them")
                        .on_hover_text("Stop using the old folder; its files stay on disk.")
                        .clicked()
                    {
                        self.forget_model_move();
                    }
                });
            });
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
            ui.checkbox(&mut self.capture_others, format!("Others ({})", platform::SYSTEM_AUDIO));
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
                        platform::reveal(&settings_file);
                    }
                });
                ui.end_row();
            });
    }
}
