//! The navigation sidebar: pages at the top, Settings pinned to the bottom.

use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Response, RichText, Sense, Ui, Vec2};

use super::{App, Page, theme};

impl App {
    pub(super) fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(2.0);
        ui.label(RichText::new("Meeting Transcriber").size(15.5).strong());
        ui.add_space(14.0);

        let recording = self.recording().then_some(theme::RECORD_RED);
        if nav_item(ui, "🎤", "Meeting", self.page == Page::Meeting, recording).clicked() {
            self.page = Page::Meeting;
        }
        // Past meetings will be listed here, between the pages and Settings.

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            if nav_item(ui, "⚙", "Settings", self.page == Page::Settings, None).clicked() {
                self.page = Page::Settings;
            }
        });
    }
}

/// A full-width, rounded navigation row with an icon and an optional dot,
/// e.g. red while recording.
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
