//! The sidebar: Meetings or Dictation, search, the meetings (or dictations)
//! grouped by day, and Settings pinned to the bottom.

use eframe::egui::{
    self, Align2, Color32, FontId, Frame, Galley, Key, Response, RichText, Sense, Ui, Vec2,
    text::{LayoutJob, TextWrapping},
};

use super::{App, Page, SidebarTab, platform, theme};
use crate::meetings::{self, Entry, Kind};

/// What a row asked for; applied after the list is drawn.
enum RowAction {
    Open(Entry),
    StartRename(Entry),
    Rename(Entry, String),
    CancelRename,
    AskDelete(Entry),
    Delete(Entry),
    CancelDelete,
}

impl App {
    pub(super) fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(2.0);
        ui.label(RichText::new("Meeting Transcriber").size(15.5).strong());
        ui.add_space(10.0);
        self.tab_switch(ui);
        ui.add_space(8.0);

        let dictation = self.sidebar_tab == SidebarTab::Dictation;
        if !dictation {
            let recording = self.recording();
            let new = egui::Button::new(RichText::new("＋  New meeting").strong())
                .min_size(Vec2::new(ui.available_width(), 32.0))
                .corner_radius(8);
            if ui
                .add_enabled(!recording, new)
                .on_disabled_hover_text("Stop recording to start another meeting.")
                .clicked()
            {
                self.new_meeting();
            }
            ui.add_space(6.0);
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.list.search)
                .hint_text(if dictation {
                    "🔍  Search dictations"
                } else {
                    "🔍  Search titles and #tags"
                })
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);

        egui::Panel::bottom("sidebar_bottom")
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new())
            .show(ui, |ui| {
                ui.add_space(6.0);
                if nav_item(ui, "⚙", "Settings", self.page == Page::Settings, None).clicked() {
                    self.page = Page::Settings;
                }
            });
        egui::ScrollArea::vertical()
            .id_salt(if dictation { "dictation_list" } else { "meeting_list" })
            .auto_shrink(false)
            .show(ui, |ui| {
                if dictation {
                    self.dictation_list(ui);
                } else {
                    self.meeting_list(ui);
                }
            });
    }

    /// Meetings | Dictation, as two halves of one control.
    fn tab_switch(&mut self, ui: &mut Ui) {
        let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
        ui.horizontal(|ui| {
            for (tab, label, page) in [
                (SidebarTab::Meetings, "Meetings", Page::Meeting),
                (SidebarTab::Dictation, "Dictation", Page::Dictation),
            ] {
                let selected = self.sidebar_tab == tab;
                let mut text = RichText::new(label).size(13.5);
                if selected {
                    text = text.strong().color(theme::accent(ui));
                }
                let button = egui::Button::new(text)
                    .fill(if selected {
                        theme::accent(ui).gamma_multiply(0.18)
                    } else {
                        Color32::TRANSPARENT
                    })
                    .min_size(Vec2::new(width, 28.0))
                    .corner_radius(7);
                if ui.add(button).clicked() {
                    if !selected {
                        self.sidebar_tab = tab;
                        self.list.search.clear();
                    }
                    self.page = page;
                }
            }
        });
    }

    fn meeting_list(&mut self, ui: &mut Ui) {
        let recording = self.recording();
        let query = self.list.search.trim().to_string();
        let on_meeting_page = self.page == Page::Meeting;

        // The open meeting before its first save is not in the folder yet.
        if self.open.path.is_none() && query.is_empty() {
            let title = self.open.meeting.title.clone();
            let dot = recording.then_some(theme::RECORD_RED);
            if list_row(ui, &title, "Not saved yet", false, on_meeting_page, dot, true).clicked() {
                self.page = Page::Meeting;
            }
            ui.add_space(4.0);
        }

        let today = chrono::Local::now().date_naive();
        let mut group = None;
        let mut action = None;
        let entries: Vec<Entry> = self
            .list
            .library
            .entries
            .iter()
            .filter(|e| e.matches(&query))
            .cloned()
            .collect();
        if entries.is_empty() && !query.is_empty() {
            ui.weak("No meetings match.");
        }
        for entry in entries {
            let label = meetings::group_label(entry.date.date(), today);
            if group.as_ref() != Some(&label) {
                ui.add_space(if group.is_some() { 10.0 } else { 2.0 });
                ui.label(RichText::new(&label).size(11.5).strong().weak());
                ui.add_space(2.0);
                group = Some(label);
            }
            if let Some(a) = self.entry_row(ui, &entry, recording, on_meeting_page) {
                action = Some(a);
            }
        }
        if let Some(action) = action {
            self.apply_row_action(action);
        }
    }

    fn entry_row(&mut self, ui: &mut Ui, entry: &Entry, recording: bool, on_meeting_page: bool) -> Option<RowAction> {
        let is_open = self.open.path.as_ref() == Some(&entry.path);
        if let Some((path, title)) = &mut self.list.renaming
            && *path == entry.path
        {
            return rename_field(ui, entry, title);
        }

        let mut detail = entry.date.format("%H:%M").to_string();
        if entry.kind == Kind::Text {
            detail += "  ·  txt";
        }
        if !entry.tags.is_empty() {
            detail += &format!("  ·  {}", entry.tags.join(", "));
        }
        // While recording, only the meeting being recorded can be shown.
        let enabled = !recording || is_open;
        let dot = (recording && is_open).then_some(theme::RECORD_RED);
        let response = list_row(
            ui,
            &entry.title,
            &detail,
            entry.kind == Kind::Text,
            is_open && on_meeting_page,
            dot,
            enabled,
        );
        let response = if enabled {
            response
        } else {
            response.on_hover_text("Stop recording to open another meeting.")
        };
        let mut action = None;
        if response.clicked() && enabled {
            action = Some(RowAction::Open(entry.clone()));
        }
        let busy = recording && is_open;
        response.context_menu(|ui| {
            let markdown = entry.kind == Kind::Markdown;
            if ui
                .add_enabled(markdown && !busy, egui::Button::new("Rename"))
                .on_disabled_hover_text(if markdown {
                    "Stop recording first."
                } else {
                    "Convert to Markdown to rename."
                })
                .clicked()
            {
                action = Some(RowAction::StartRename(entry.clone()));
            }
            if ui.button(platform::SHOW_IN_FILES).clicked() {
                platform::reveal(&entry.path);
            }
            ui.separator();
            if ui
                .add_enabled(!busy, egui::Button::new("Delete…"))
                .on_disabled_hover_text("Stop recording first.")
                .clicked()
            {
                action = Some(RowAction::AskDelete(entry.clone()));
            }
        });
        if self.list.confirm_delete.as_ref() == Some(&entry.path) {
            action = confirm_delete(ui, entry).or(action);
        }
        action
    }

    fn apply_row_action(&mut self, action: RowAction) {
        match action {
            RowAction::Open(e) => self.open_meeting(&e.path, e.kind),
            RowAction::StartRename(e) => {
                let title = if self.open.path.as_ref() == Some(&e.path) {
                    self.open.meeting.title.clone()
                } else {
                    e.title.clone()
                };
                self.list.renaming = Some((e.path, title));
            }
            RowAction::Rename(e, title) => {
                self.list.renaming = None;
                if title.trim() != e.title {
                    self.rename_meeting(&e.path, &title);
                }
            }
            RowAction::CancelRename => self.list.renaming = None,
            RowAction::AskDelete(e) => self.list.confirm_delete = Some(e.path),
            RowAction::Delete(e) => self.delete_meeting(&e.path),
            RowAction::CancelDelete => self.list.confirm_delete = None,
        }
    }
}

/// The inline title field of a meeting being renamed. Return or clicking
/// away renames; Escape cancels.
fn rename_field(ui: &mut Ui, entry: &Entry, title: &mut String) -> Option<RowAction> {
    let response = ui.add(
        egui::TextEdit::singleline(title)
            .id_salt(("rename", &entry.path))
            .desired_width(f32::INFINITY),
    );
    if !response.has_focus() && !response.lost_focus() {
        response.request_focus();
    }
    if response.lost_focus() {
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            return Some(RowAction::CancelRename);
        }
        return Some(RowAction::Rename(entry.clone(), title.clone()));
    }
    None
}

/// The in-app "are you sure" under a row about to be deleted.
fn confirm_delete(ui: &mut Ui, entry: &Entry) -> Option<RowAction> {
    let red = theme::RECORD_RED;
    let mut action = None;
    Frame::new()
        .fill(red.gamma_multiply(0.1))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Delete this meeting?").strong());
            ui.weak("Its file is removed from the folder.");
            ui.horizontal(|ui| {
                let delete = egui::Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(red);
                if ui.add(delete).clicked() {
                    action = Some(RowAction::Delete(entry.clone()));
                }
                if ui.button("Cancel").clicked() {
                    action = Some(RowAction::CancelDelete);
                }
            });
        });
    ui.add_space(4.0);
    action
}

/// A meeting (or a dictation) in the list: its title on one line, time
/// and tags below.
pub(super) fn list_row(
    ui: &mut Ui,
    title: &str,
    detail: &str,
    faded: bool,
    selected: bool,
    dot: Option<Color32>,
    enabled: bool,
) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), sense);
    let accent = theme::accent(ui);
    let visuals = ui.visuals();
    let fill = if selected {
        accent.gamma_multiply(0.18)
    } else if response.hovered() && enabled {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let mut text = if selected { accent } else { visuals.text_color() };
    if faded || !enabled {
        text = text.gamma_multiply(0.7);
    }
    let weak = visuals.weak_text_color();
    let width = rect.width() - if dot.is_some() { 40.0 } else { 22.0 };
    let title = one_line_galley(ui, title, FontId::proportional(14.0), text, width);
    let detail = one_line_galley(ui, detail, FontId::proportional(11.5), weak, width);
    let painter = ui.painter();
    painter.rect_filled(rect, 8, fill);
    painter.galley(rect.left_top() + Vec2::new(11.0, 6.0), title, text);
    painter.galley(rect.left_top() + Vec2::new(11.0, 25.0), detail, weak);
    if let Some(color) = dot {
        painter.circle_filled(rect.right_center() - Vec2::new(14.0, 0.0), 4.0, color);
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Text cut to one line with an ellipsis.
fn one_line_galley(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}

/// A full-width, rounded navigation row with an icon and an optional dot.
fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool, dot: Option<Color32>) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
    let accent = theme::accent(ui);
    let visuals = ui.visuals();
    let fill = if selected {
        accent.gamma_multiply(0.18)
    } else if response.hovered() {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let text = if selected { accent } else { visuals.text_color() };
    let painter = ui.painter();
    painter.rect_filled(rect, 8, fill);
    let left = rect.left_center();
    painter.text(
        left + Vec2::new(14.0, 0.0),
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(15.0),
        text,
    );
    painter.text(
        left + Vec2::new(30.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.5),
        text,
    );
    if let Some(color) = dot {
        painter.circle_filled(rect.right_center() - Vec2::new(14.0, 0.0), 4.0, color);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
