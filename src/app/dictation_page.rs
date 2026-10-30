//! Dictation on screen: the banners (voice2cursor running, no hotkey
//! permission), the history in the sidebar, the Dictation page with one
//! clip, and the Dictation card in Settings.

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui};

use super::dictation::describe;
use super::sidebar::list_row;
use super::{App, Folder, LANGUAGES, Page, language_name, platform, theme};
use crate::dictation::archive::Clip;
use crate::dictation::hotkey::{self, Hotkey};
use crate::dictation::{Phase, voice2cursor};
use crate::meetings;

/// How many more clips "Show more" lists.
pub(super) const LIST_STEP: usize = 200;

/// A tinted banner with a message and buttons.
fn banner(ui: &mut Ui, color: Color32, text: &str, buttons: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .corner_radius(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(text).color(color));
                buttons(ui);
            });
        });
    ui.add_space(8.0);
}

fn seconds_text(seconds: Option<f32>) -> String {
    match seconds {
        Some(s) if s >= 60.0 => format!("{}:{:02}", s as u32 / 60, s as u32 % 60),
        Some(s) => format!("{s:.1} s"),
        None => "no recording".into(),
    }
}

impl App {
    /// Dictation's banners, above every page.
    pub(super) fn dictation_banners(&mut self, ui: &mut Ui) {
        let warn = ui.visuals().warn_fg_color;
        if self.dict.voice2cursor_banner {
            let mut resume = false;
            let mut dismiss = false;
            banner(ui, warn, &voice2cursor::banner(), |ui| {
                resume = ui.small_button("Resume dictation").clicked();
                dismiss = ui.small_button("Dismiss").clicked();
            });
            if resume {
                self.set_dictation_paused(false);
            }
            if dismiss {
                self.dict.voice2cursor_banner = false;
            }
        }
        // Where dictation is not supported yet, the Dictation page says so.
        if let Some(why) = self.dict.hotkey_error.clone()
            && self.dict.settings.enabled
            && platform::DICTATION
        {
            let mut retry = false;
            let mut dismiss = false;
            banner(ui, warn, &why, |ui| {
                retry = ui.small_button("Try again").clicked();
                if ui.small_button("Open System Settings").clicked() {
                    platform::open_hotkey_settings();
                }
                dismiss = ui.small_button("Dismiss").clicked();
            });
            if retry {
                self.start_hotkey();
            }
            if dismiss {
                self.dict.hotkey_error = None;
            }
        }
    }

    /// The dictation history in the sidebar, grouped by day.
    pub(super) fn dictation_list(&mut self, ui: &mut Ui) {
        self.refresh_clips();
        let query = self.list.search.trim().to_string();
        let today = chrono::Local::now().date_naive();
        let on_page = self.page == Page::Dictation;
        let mut open = None;
        let clips: Vec<&Clip> = self.dict.clips.clips.iter().filter(|c| c.matches(&query)).collect();
        if clips.is_empty() {
            ui.weak(if query.is_empty() {
                "No dictations yet."
            } else {
                "No dictations match."
            });
        }
        let mut group = None;
        // The folder can hold thousands of clips; drawing them all would
        // slow every frame.
        let shown = clips.len().min(self.dict.list_limit);
        for clip in &clips[..shown] {
            let label = meetings::group_label(clip.at.date(), today);
            if group.as_ref() != Some(&label) {
                ui.add_space(if group.is_some() { 10.0 } else { 2.0 });
                ui.label(RichText::new(&label).size(11.5).strong().weak());
                ui.add_space(2.0);
                group = Some(label);
            }
            let text = if clip.text.is_empty() { "(no text)" } else { &clip.text };
            let detail = format!("{}  ·  {}", clip.at.format("%H:%M"), seconds_text(clip.seconds));
            let selected = on_page && self.dict.selected.as_deref() == Some(&clip.stem);
            let response = list_row(ui, text, &detail, clip.text.is_empty(), selected, None, true);
            let response = response.on_hover_text(text);
            if response.clicked() {
                open = Some(clip.stem.clone());
            }
            response.context_menu(|ui| clip_actions(ui, clip));
        }
        let more = clips.len() - shown;
        if more > 0 {
            ui.add_space(6.0);
            if ui.button(format!("Show {} more", more.min(LIST_STEP))).clicked() {
                self.dict.list_limit += LIST_STEP;
            }
        }
        if let Some(stem) = open {
            self.dict.selected = Some(stem);
            self.page = Page::Dictation;
        }
    }

    pub(super) fn dictation_page(&mut self, ui: &mut Ui) {
        let status = self.dict.status();
        let key = self.dict.settings.hotkey().label();
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let color = match status.phase() {
                    Phase::Recording => theme::RECORD_RED,
                    Phase::Ready => theme::READY_GREEN,
                    Phase::Error => ui.visuals().warn_fg_color,
                    _ => ui.visuals().weak_text_color(),
                };
                ui.label(RichText::new("●").color(color).size(16.0));
                ui.label(RichText::new(describe(&status, &key)).size(16.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if self.dict.paused { "Resume" } else { "Pause" };
                    if ui
                        .add_enabled(self.dict.settings.enabled, egui::Button::new(label))
                        .clicked()
                    {
                        self.set_dictation_paused(!self.dict.paused);
                    }
                    if status.last_error.is_some() && ui.button("Dismiss error").clicked() {
                        self.dict.dictation.clear_error();
                    }
                });
            });
            ui.weak(format!(
                "Hold {key} anywhere and speak; let go and the text is pasted where the cursor is ({}).",
                platform::PASTE_SHORTCUT
            ));
            if !platform::DICTATION {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Dictation is not supported on this system yet. Recordings from the folder are listed.",
                );
            } else if !self.dict.settings.enabled {
                ui.weak("Turn dictation on in Settings.");
            }
        });
        ui.add_space(12.0);
        let selected = self
            .dict
            .selected
            .as_ref()
            .and_then(|stem| self.dict.clips.clips.iter().find(|c| &c.stem == stem))
            .cloned();
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(clip) = selected else {
                ui.weak("Pick a dictation in the sidebar to read, play or copy it.");
                return;
            };
            ui.label(
                RichText::new(clip.at.format("%A, %B %-d, %Y  ·  %H:%M:%S").to_string())
                    .strong()
                    .size(15.0),
            );
            ui.weak(seconds_text(clip.seconds));
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .id_salt("clip_text")
                .max_height(ui.available_height() - 48.0)
                .show(ui, |ui| {
                    let text = if clip.text.is_empty() {
                        "No text was recognized."
                    } else {
                        &clip.text
                    };
                    ui.add(egui::Label::new(RichText::new(text).size(15.0)).wrap().selectable(true));
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!clip.text.is_empty(), egui::Button::new("Copy"))
                    .clicked()
                {
                    ui.ctx().copy_text(clip.text.clone());
                    self.notify("Copied".into(), None);
                }
                if let Some(wav) = &clip.wav {
                    if ui.button("▶  Play").clicked() {
                        platform::open_file(wav);
                    }
                    if ui.button(platform::SHOW_IN_FILES).clicked() {
                        platform::reveal(wav);
                    }
                }
                if let Some(notice) = self.notice() {
                    ui.weak(&notice.text);
                }
            });
        });
    }

    /// The Dictation card in Settings.
    pub(super) fn dictation_card(&mut self, ui: &mut Ui) {
        theme::card_title(ui, "Dictation");
        ui.weak(
            "Hold the hotkey in any app and speak; let go and the text is typed where the cursor is. \
             It uses the model chosen above, so it is only loaded once.",
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.dict.settings.enabled, "Dictation");
            if self.dict.settings.enabled {
                let mut paused = self.dict.paused;
                if ui.checkbox(&mut paused, "Paused").changed() {
                    self.set_dictation_paused(paused);
                }
            }
        });
        ui.add_enabled_ui(self.dict.settings.enabled, |ui| {
            self.hotkey_row(ui);
            ui.horizontal(|ui| {
                ui.label("Language");
                let mut language = LANGUAGES
                    .iter()
                    .find(|(code, _)| code.is_some_and(|c| c == self.dict.settings.language))
                    .and_then(|(code, _)| *code);
                egui::ComboBox::from_id_salt("dictation_language")
                    .selected_text(language_name(language))
                    .show_ui(ui, |ui| {
                        for (code, name) in LANGUAGES {
                            ui.selectable_value(&mut language, *code, *name);
                        }
                    });
                self.dict.settings.language = language.unwrap_or_default().to_string();
            });
            ui.horizontal(|ui| {
                ui.label("Initial prompt");
                ui.add(
                    egui::TextEdit::singleline(&mut self.dict.settings.initial_prompt)
                        .hint_text("e.g. 以下是普通话的句子，使用简体中文。")
                        .desired_width(f32::INFINITY),
                );
            });
            ui.weak("Optional. Names and the writing style you want, e.g. simplified Chinese with English terms.");
            ui.add_space(6.0);
            ui.checkbox(
                &mut self.dict.settings.auto_paste,
                format!("Paste automatically ({})", platform::PASTE_SHORTCUT),
            );
            ui.weak("Off: the text is only copied to the clipboard.");
            ui.checkbox(&mut self.dict.settings.sounds, "Sounds (start, done, error)");
            ui.horizontal(|ui| {
                ui.label("Ignore clips shorter than");
                ui.add(
                    egui::DragValue::new(&mut self.dict.settings.min_seconds)
                        .range(0.0..=5.0)
                        .speed(0.05)
                        .suffix(" s"),
                );
                ui.label("Stop after");
                ui.add(
                    egui::DragValue::new(&mut self.dict.settings.max_seconds)
                        .range(5.0..=1800.0)
                        .speed(5.0)
                        .suffix(" s"),
                );
            });
            ui.add_space(6.0);
            ui.checkbox(&mut self.dict.settings.save_recordings, "Keep recordings");
            ui.weak("Each dictation is saved as a WAV with its text beside it (about 2 MB a minute), and listed in the sidebar.");
            if self.dict.settings.save_recordings {
                self.folder_row(ui, Folder::Recordings);
            }
            ui.add_space(6.0);
            ui.checkbox(&mut self.dict.settings.ollama, "Clean up with Ollama");
            ui.weak("A local Ollama model fixes punctuation and spacing (1–3 s more per dictation). If it fails, the text is kept as it was.");
            if self.dict.settings.ollama {
                egui::Grid::new("ollama").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("URL");
                    ui.add(egui::TextEdit::singleline(&mut self.dict.settings.ollama_url).desired_width(320.0));
                    ui.end_row();
                    ui.label("Model");
                    ui.add(egui::TextEdit::singleline(&mut self.dict.settings.ollama_model).desired_width(200.0));
                    ui.end_row();
                    ui.label("Timeout");
                    ui.add(
                        egui::DragValue::new(&mut self.dict.settings.ollama_timeout)
                            .range(1.0..=120.0)
                            .suffix(" s"),
                    );
                    ui.end_row();
                });
            }
        });
        if platform::AUTOSTART {
            ui.add_space(6.0);
            let mut on = self.dict.open_at_login;
            if ui.checkbox(&mut on, "Open at login").changed() {
                match platform::set_open_at_login(on) {
                    Ok(()) => self.dict.open_at_login = on,
                    Err(e) => self.errors.push(format!("{e:#}")),
                }
            }
            ui.weak("Starts in the menu bar when you log in, from Applications.");
        }
    }

    fn hotkey_row(&mut self, ui: &mut Ui) {
        let current = self.dict.settings.hotkey();
        ui.horizontal(|ui| {
            ui.label("Hotkey");
            let mut choice = current;
            egui::ComboBox::from_id_salt("dictation_hotkey")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    for key in hotkey::PRESETS {
                        ui.selectable_value(&mut choice, *key, key.label());
                    }
                    if !hotkey::PRESETS.contains(&current) {
                        ui.selectable_value(&mut choice, current, current.label());
                    }
                });
            if choice != current {
                self.dict.settings.hotkey = choice.name();
            }
            let capturing = self.capturing_hotkey();
            if capturing {
                ui.spinner();
                ui.label("Press the key to use… (Esc cancels)");
                if ui.button("Cancel").clicked()
                    && let Some(watch) = &self.dict.hotkey
                {
                    watch.capture_next(false);
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            } else if ui
                .add_enabled(self.dict.hotkey.is_some(), egui::Button::new("Press a key…"))
                .on_disabled_hover_text("The hotkey can't be watched; see the message at the top.")
                .clicked()
                && let Some(watch) = &self.dict.hotkey
            {
                watch.capture_next(true);
            }
        });
        if matches!(current, Hotkey::Vk(_)) {
            ui.weak(format!("Saved as {}.", current.name()));
        }
    }
}

/// Right-click menu of a clip in the sidebar.
fn clip_actions(ui: &mut Ui, clip: &Clip) {
    if ui
        .add_enabled(!clip.text.is_empty(), egui::Button::new("Copy text"))
        .clicked()
    {
        ui.ctx().copy_text(clip.text.clone());
    }
    if let Some(wav) = &clip.wav {
        if ui.button("Play").clicked() {
            platform::open_file(wav);
        }
        if ui.button(platform::SHOW_IN_FILES).clicked() {
            platform::reveal(wav);
        }
    }
}
