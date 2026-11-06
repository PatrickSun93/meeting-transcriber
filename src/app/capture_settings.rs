//! The Screenshots card in Settings: the capture hotkeys (screenshot,
//! scrolling screenshot, screen recording, pin), what Enter does with a
//! plain screenshot, the countdown, where files go, the drawing tools'
//! start, the scrolling screenshot's step and longest picture, and screen
//! recording (`recording.rs`).

use std::time::Duration;

use eframe::egui::{self, Response, RichText, Ui};

use super::dictation::KeysFor;
use super::{App, Folder, platform};
use crate::capture;
use crate::capture::annotate::Tool;
use crate::capture::scrolling::MAX_HEIGHTS;
use crate::platform::{HotkeyWatch, Press};
use crate::settings::{Choice, Clash, DELAYS, DrawStyle, OFF, default_press_hotkey};

/// A capture hotkey's name in Settings.
fn title(press: Press) -> &'static str {
    match press {
        Press::Screenshot => "Screenshot",
        Press::Scroll => "Scrolling screenshot",
        Press::Record => "Screen recording",
        Press::Pin => "Pin",
    }
}

/// What a countdown of `seconds` is called.
fn delay_label(seconds: u32) -> String {
    if seconds == 0 {
        "None".into()
    } else {
        format!("{seconds} seconds")
    }
}

/// A drop-down of a setting's choices.
pub(super) fn choice_combo<T: Choice>(ui: &mut Ui, id: &str, value: &mut T) -> Response {
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.label())
        .show_ui(ui, |ui| {
            for choice in T::ALL {
                ui.selectable_value(value, *choice, choice.label());
            }
        })
        .response
}

/// A small heading inside the card.
fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).strong());
}

impl App {
    /// The Screenshots card in Settings.
    pub(super) fn screenshot_card(&mut self, ui: &mut Ui) {
        crate::app::theme::card_title(ui, "Screenshots");
        ui.weak(
            "Press the screenshot hotkey in any app (or use Screenshot in the sidebar or the menu), then drag over \
             what to keep, or click a window. Draw on it with the tools beside it, or Pin it to keep it on the \
             screen. Enter does what is chosen below: at first it copies the picture and saves it, beside the \
             open meeting with a line in its notes, or in the Screenshots folder.",
        );
        if let Some(why) = capture::unsupported() {
            ui.colored_label(ui.visuals().warn_fg_color, why);
            return;
        }
        self.capture_hotkeys(ui);

        heading(ui, "Screenshots");
        let settings = &mut self.capture.settings;
        ui.horizontal(|ui| {
            ui.label("Enter, a double-click and Copy");
            choice_combo(ui, "screenshot_action", &mut settings.action).on_hover_text(
                "What a plain screenshot does when you press Enter or double-click it, and whether Copy saves it \
                 too. The scrolling screenshot, screen recording and pin hotkeys do their own thing.",
            );
        });
        ui.horizontal(|ui| {
            ui.label("Countdown");
            egui::ComboBox::from_id_salt("capture_delay")
                .selected_text(delay_label(settings.delay))
                .show_ui(ui, |ui| {
                    for seconds in DELAYS {
                        ui.selectable_value(&mut settings.delay, seconds, delay_label(seconds));
                    }
                })
                .response
                .on_hover_text(
                    "Waits before the screen is taken, counting down at the top of the screen (and in the menu \
                     bar or tray), to open a menu first. The hotkey again cancels it.",
                );
        });

        heading(ui, "Where they go");
        ui.checkbox(
            &mut settings.into_meeting,
            "Into the open meeting, beside its file, with a line in its notes",
        );
        if !settings.into_meeting {
            ui.weak("Screenshots and screen recordings always go to the folders below; the 📷 above the notes still puts one into its meeting.");
        }
        ui.add_space(4.0);
        self.folder_row(ui, Folder::Screenshots);
        if capture::record::unsupported().is_none() {
            ui.add_space(4.0);
            self.folder_row(ui, Folder::ScreenRecordings);
        }

        heading(ui, "Drawing");
        let settings = &mut self.capture.settings;
        ui.horizontal(|ui| {
            ui.label("Tool at first");
            let shown = settings.draw_tool.map_or("None", Choice::label);
            egui::ComboBox::from_id_salt("draw_tool")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.draw_tool, None, "None");
                    for tool in Tool::ALL {
                        ui.selectable_value(&mut settings.draw_tool, Some(tool), tool.label());
                    }
                })
                .response
                .on_hover_text("The drawing tool in use when the overlay opens. None: drags change the selection.");
        });
        ui.horizontal(|ui| {
            ui.weak("Each tool keeps the colour and size it was last used with.");
            if settings.draw_style != DrawStyle::default() && ui.small_button("Reset").clicked() {
                settings.draw_style = DrawStyle::default();
            }
        });

        heading(ui, "Scrolling screenshot");
        ui.horizontal(|ui| {
            ui.label("Step");
            choice_combo(ui, "scroll_step", &mut settings.scroll_step).on_hover_text(
                "How far each turn of the wheel scrolls at first. Smaller is slower and surer on pages that jump.",
            );
        });
        ui.horizontal(|ui| {
            ui.label("Longest picture");
            ui.add(
                egui::DragValue::new(&mut settings.scroll_max_height)
                    .range(MAX_HEIGHTS)
                    .speed(100.0)
                    .suffix(" px"),
            )
            .on_hover_text("It stops by itself at this height.");
        });

        self.recording_settings(ui);
    }

    /// The capture hotkeys, each with Record keys…, Reset and Off.
    fn capture_hotkeys(&mut self, ui: &mut Ui) {
        heading(ui, "Hotkeys");
        ui.weak(
            "Each opens the same overlay with its own main action, which Enter and a double-click do. The screen \
             recording hotkey (like any capture hotkey) also stops a recording.",
        );
        for press in Press::ALL {
            self.capture_hotkey_row(ui, press);
        }
        if self.dict.hotkey.is_none() {
            ui.weak(format!(
                "The hotkeys work once the keyboard can be watched. {}",
                platform::HOTKEY_HINT
            ));
        } else if self.dict.hotkey.as_ref().and_then(HotkeyWatch::system_key).is_some() {
            ui.weak("The desktop does not let apps watch the keyboard here, so only the Screenshot buttons work.");
        }
    }

    fn capture_hotkey_row(&mut self, ui: &mut Ui, press: Press) {
        let current = self.capture.settings.press_hotkey(press);
        let default = default_press_hotkey(press);
        let capturing = self.capturing_keys(KeysFor::Capture(press));
        let mut value = None;
        ui.horizontal(|ui| {
            ui.label(title(press));
            let shown = current.as_ref().map_or_else(|| "Off".to_string(), |k| k.label());
            ui.label(RichText::new(shown).strong());
            if capturing {
                ui.spinner();
                ui.label("Press the keys to use, then let go… (Esc cancels)");
                if ui.button("Cancel").clicked()
                    && let Some(watch) = &self.dict.hotkey
                {
                    watch.capture_next(false);
                }
                ui.ctx().request_repaint_after(Duration::from_millis(200));
                return;
            }
            if ui
                .add_enabled(self.dict.hotkey.is_some(), egui::Button::new("Record keys…"))
                .on_hover_text("Hold a combination, e.g. Ctrl+Shift+2, then let go.")
                .on_disabled_hover_text("The keyboard can't be watched; see the message at the top.")
                .clicked()
            {
                self.record_keys(KeysFor::Capture(press));
            }
            if current != default && ui.button("Reset").clicked() {
                value = Some(default.as_ref().map_or_else(|| OFF.to_string(), |k| k.name()));
            }
            if current.is_some() && ui.button("Off").clicked() {
                value = Some(OFF.to_string());
            }
        });
        if let Some(value) = value {
            *self.capture.settings.press_value_mut(press) = value;
        }
        let warn = ui.visuals().warn_fg_color;
        match self.capture.settings.clash(press, &self.dict.settings.hotkey()) {
            Some(Clash::Dictation) => {
                ui.colored_label(warn, "This is also the dictation hotkey: holding it dictates too.");
            }
            Some(Clash::Press(first)) => {
                ui.colored_label(
                    warn,
                    format!(
                        "The same keys as the {} hotkey, which goes first: this one does nothing.",
                        first.label()
                    ),
                );
            }
            None => {}
        }
    }
}
