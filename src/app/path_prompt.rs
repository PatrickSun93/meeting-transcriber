//! Asks for a folder as text when there is no folder dialog to show: on
//! Linux, when neither zenity nor kdialog is installed.

use std::path::Path;

use eframe::egui::{self, RichText};

use super::library::Format;
use super::{App, Folder, platform};
use crate::settings;

/// What the typed folder is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptFor {
    Folder(Folder),
    Export(Format),
}

/// The open "type a folder" dialog.
pub(super) struct PathPrompt {
    target: PromptFor,
    title: String,
    text: String,
    /// Focus goes to the text field once, when the dialog opens.
    focused: bool,
}

impl PathPrompt {
    pub(super) fn new(target: PromptFor, title: &str, start: &Path) -> Self {
        Self {
            target,
            title: title.to_string(),
            text: start.display().to_string(),
            focused: false,
        }
    }
}

impl App {
    /// Acts on what a folder dialog came back with. With no dialog to show,
    /// asks for the path as text, starting from `start`.
    pub(super) fn picked(&mut self, target: PromptFor, title: &str, start: &Path, picked: platform::Picked) {
        match picked {
            platform::Picked::Folder(dir) => self.use_typed_or_picked(target, &dir.to_string_lossy()),
            platform::Picked::Cancelled => {}
            platform::Picked::NoDialog => self.path_prompt = Some(PathPrompt::new(target, title, start)),
        }
    }

    /// Applies a chosen folder: a setting keeps the text as written (so `~`
    /// stays portable), an export needs the real path.
    fn use_typed_or_picked(&mut self, target: PromptFor, text: &str) {
        match target {
            PromptFor::Folder(folder) => self.set_folder(folder, text.trim().to_string()),
            PromptFor::Export(format) => {
                if let Some(dir) = settings::expand_dir(text) {
                    self.export_to(format, &dir);
                }
            }
        }
    }

    /// Draws the dialog while it is open.
    pub(super) fn path_prompt_dialog(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.path_prompt else { return };
        let mut ok = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::unique("path prompt")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(&prompt.title).strong());
            ui.weak(platform::NO_DIALOG_HINT);
            ui.weak("~ stands for your home folder.");
            ui.add_space(6.0);
            let field = ui.add(egui::TextEdit::singleline(&mut prompt.text).desired_width(f32::INFINITY));
            if !prompt.focused {
                field.request_focus();
                prompt.focused = true;
            }
            let filled = settings::expand_dir(&prompt.text).is_some();
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && filled {
                ok = true;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ok |= ui.add_enabled(filled, egui::Button::new("OK")).clicked();
                cancel |= ui.button("Cancel").clicked();
            });
        });
        if ok {
            let (target, text) = (prompt.target, prompt.text.clone());
            self.path_prompt = None;
            self.use_typed_or_picked(target, &text);
        } else if cancel || response.should_close() {
            self.path_prompt = None;
        }
    }
}
