//! The transcript reader, for the live meeting and past ones alike: a
//! readable column with timestamps and speaker pills, a Me/Others filter,
//! find-in-transcript, and copying single lines.

use std::ops::Range;

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Layout, Modifiers, RichText, Sense, TextFormat, TextStyle, Ui, Vec2,
    text::LayoutJob,
};

use super::{App, theme};
use crate::meeting::find_matches;
use crate::transcript::{Line, Speaker, format_time};

/// Lines longer than this are hard to follow; wider windows get margins.
const READABLE_WIDTH: f32 = 760.0;
const LINE_GAP: f32 = 7.0;
/// Line height as a multiple of the font size.
const LINE_HEIGHT: f32 = 1.45;

/// One find result: a byte range in the text of a transcript line.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Match {
    line: usize,
    range: Range<usize>,
}

pub(super) struct Reader {
    /// Show only one speaker.
    pub filter: Option<Speaker>,
    pub find: String,
    /// The highlighted match, an index into `matches`.
    current: usize,
    scroll_to_current: bool,
    pub notes_open: bool,
    matches: Vec<Match>,
    /// What `matches` was computed for: query, filter, line count, version.
    matches_for: Option<(String, Option<Speaker>, usize, u64)>,
    /// Bumped when the lines are replaced, so matches are found again.
    version: u64,
}

impl Default for Reader {
    fn default() -> Self {
        Self {
            filter: None,
            find: String::new(),
            current: 0,
            scroll_to_current: false,
            notes_open: true,
            matches: Vec::new(),
            matches_for: None,
            version: 0,
        }
    }
}

impl Reader {
    /// Another meeting was opened. The search text is kept, so the same
    /// words can be looked up in several meetings.
    pub fn opened(&mut self) {
        self.filter = None;
        self.current = 0;
        self.scroll_to_current = !self.find.is_empty();
        self.lines_replaced();
    }

    pub fn lines_replaced(&mut self) {
        self.version += 1;
    }

    fn update_matches(&mut self, lines: &[Line]) {
        let key = (self.find.clone(), self.filter, lines.len(), self.version);
        if self.matches_for.as_ref() == Some(&key) {
            return;
        }
        self.matches = transcript_matches(lines, self.filter, &self.find);
        if self.current >= self.matches.len() {
            self.current = 0;
        }
        self.matches_for = Some(key);
    }

    fn step(&mut self, forward: bool) {
        self.current = step_index(self.current, self.matches.len(), forward);
        self.scroll_to_current = true;
    }
}

/// Every match of `query` in the lines `filter` lets through, in order.
pub(super) fn transcript_matches(lines: &[Line], filter: Option<Speaker>, query: &str) -> Vec<Match> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| filter.is_none_or(|s| s == l.speaker))
        .flat_map(|(i, l)| {
            find_matches(&l.text, query)
                .into_iter()
                .map(move |range| Match { line: i, range })
        })
        .collect()
}

/// Next or previous of `len` matches, wrapping around.
fn step_index(current: usize, len: usize, forward: bool) -> usize {
    match len {
        0 => 0,
        n if forward => (current + 1) % n,
        n => (current + n - 1) % n,
    }
}

/// `[00:03] Me: text`, as one line is copied.
fn line_text(line: &Line) -> String {
    format!("[{}] {}: {}", format_time(line.start), line.speaker.label(), line.text)
}

impl App {
    pub(super) fn transcript_view(&mut self, ui: &mut Ui) {
        self.reader.update_matches(self.transcript.lines());
        self.reader_toolbar(ui);
        ui.add_space(4.0);
        ui.separator();
        // An id per meeting, so each keeps its own scroll position.
        let id = ("reader", self.open.path.clone());
        let live = self.recording();
        egui::ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink(false)
            .stick_to_bottom(live)
            .show(ui, |ui| {
                let width = ui.available_width().min(READABLE_WIDTH);
                let indent = ((ui.available_width() - width) / 2.0).max(0.0);
                ui.horizontal_top(|ui| {
                    ui.add_space(indent);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.add_space(6.0);
                        self.reader_lines(ui);
                    });
                });
            });
        self.reader.scroll_to_current = false;
    }

    fn reader_toolbar(&mut self, ui: &mut Ui) {
        // Wraps in narrow windows rather than drawing over itself.
        ui.horizontal_wrapped(|ui| {
            for (value, label) in [
                (None, "All"),
                (Some(Speaker::Me), "Me"),
                (Some(Speaker::Others), "Others"),
            ] {
                if ui.selectable_label(self.reader.filter == value, label).clicked() {
                    self.reader.filter = value;
                }
            }
            ui.add_space(8.0);
            self.find_box(ui);
            let toggle = |ui: &mut Ui, open: &mut bool| {
                ui.toggle_value(open, "📝  Notes")
                    .on_hover_text("Show or hide the notes next to the transcript");
            };
            if ui.available_size_before_wrap().x > 110.0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    toggle(ui, &mut self.reader.notes_open)
                });
            } else {
                toggle(ui, &mut self.reader.notes_open);
            }
        });
    }

    /// The find field (⌘F), match count and previous/next. Return goes to
    /// the next match, Shift-Return to the previous one, Escape clears.
    fn find_box(&mut self, ui: &mut Ui) {
        let focus = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F));
        let edit = egui::TextEdit::singleline(&mut self.reader.find)
            .id_salt("find_in_transcript")
            .hint_text("Find in transcript  ⌘F")
            .desired_width(170.0);
        let response = ui.add(edit);
        if focus {
            response.request_focus();
        }
        if response.changed() {
            self.reader.current = 0;
            self.reader.scroll_to_current = true;
        }
        if response.lost_focus() {
            let (enter, shift, escape) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape)));
            if enter {
                self.reader.step(!shift);
                response.request_focus();
            } else if escape {
                self.reader.find.clear();
            }
        }
        if self.reader.find.is_empty() {
            return;
        }
        let total = self.reader.matches.len();
        if total == 0 {
            ui.weak("No matches");
            return;
        }
        ui.weak(format!("{} of {total}", self.reader.current + 1));
        if ui.small_button("⏶").on_hover_text("Previous match (⇧↩)").clicked() {
            self.reader.step(false);
        }
        if ui.small_button("⏷").on_hover_text("Next match (↩)").clicked() {
            self.reader.step(true);
        }
    }

    fn reader_lines(&mut self, ui: &mut Ui) {
        let filter = self.reader.filter;
        let shown = |s: Speaker| filter.is_none_or(|f| f == s);
        let lines = self.transcript.lines();
        let previews: Vec<_> = [Speaker::Me, Speaker::Others]
            .into_iter()
            .filter(|s| shown(*s))
            .filter_map(|s| self.transcript.preview(s).map(|p| (s, p.start, p.text.clone())))
            .collect();
        if !lines.iter().any(|l| shown(l.speaker)) && previews.is_empty() {
            empty_state(ui, filter, lines.is_empty());
            return;
        }

        let matches = &self.reader.matches;
        let current = matches.get(self.reader.current);
        let mut copy = None;
        let mut next_match = 0;
        for (i, line) in lines.iter().enumerate() {
            if !shown(line.speaker) {
                continue;
            }
            let first = next_match;
            while next_match < matches.len() && matches[next_match].line == i {
                next_match += 1;
            }
            let hits = &matches[first..next_match];
            let current_here = current.filter(|m| m.line == i).map(|m| m.range.clone());
            let row = ui.horizontal_top(|ui| {
                if timestamp(ui, line.start).on_hover_text("Copy this line").clicked() {
                    copy = Some(line_text(line));
                }
                theme::speaker_pill(ui, line.speaker, false);
                let job = highlighted(ui, &line.text, hits, current_here.as_ref());
                ui.add(egui::Label::new(job).wrap().selectable(true))
                    .context_menu(|ui| {
                        if ui.button("Copy line").clicked() {
                            copy = Some(line_text(line));
                        }
                        if ui.button("Copy text only").clicked() {
                            copy = Some(line.text.clone());
                        }
                    });
            });
            if self.reader.scroll_to_current && current_here.is_some() {
                row.response.scroll_to_me(Some(Align::Center));
            }
            ui.add_space(LINE_GAP);
        }
        for (speaker, start, text) in previews {
            ui.horizontal_top(|ui| {
                timestamp(ui, start);
                theme::speaker_pill(ui, speaker, true);
                ui.add(egui::Label::new(RichText::new(text).italics().weak()).wrap());
            });
            ui.add_space(LINE_GAP);
        }
        if let Some(text) = copy {
            ui.ctx().copy_text(text);
        }
    }
}

/// Line text with find results tinted, the current one more strongly.
fn highlighted(ui: &Ui, text: &str, hits: &[Match], current: Option<&Range<usize>>) -> LayoutJob {
    let font = TextStyle::Body.resolve(ui.style());
    let normal = TextFormat {
        line_height: Some(font.size * LINE_HEIGHT),
        font_id: font,
        color: ui.visuals().text_color(),
        ..Default::default()
    };
    let hit = TextFormat {
        background: theme::FIND_HIGHLIGHT.gamma_multiply(0.35),
        ..normal.clone()
    };
    let strong = TextFormat {
        background: theme::FIND_HIGHLIGHT,
        color: Color32::BLACK,
        ..normal.clone()
    };
    let mut job = LayoutJob::default();
    let mut at = 0;
    for m in hits {
        job.append(&text[at..m.range.start], 0.0, normal.clone());
        let format = if current == Some(&m.range) { &strong } else { &hit };
        job.append(&text[m.range.clone()], 0.0, format.clone());
        at = m.range.end;
    }
    job.append(&text[at..], 0.0, normal);
    job
}

/// A small fixed-width monospace time, so rows line up. Clickable.
fn timestamp(ui: &mut Ui, seconds: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(56.0, 21.0), Sense::click());
    let color = if response.hovered() {
        theme::accent(ui)
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        format_time(seconds),
        FontId::monospace(12.0),
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn empty_state(ui: &mut Ui, filter: Option<Speaker>, no_lines: bool) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        if let (Some(speaker), false) = (filter, no_lines) {
            ui.label(
                RichText::new(format!("No lines from {}", speaker.label()))
                    .size(17.0)
                    .strong()
                    .weak(),
            );
            return;
        }
        ui.label(RichText::new("No transcript yet").size(17.0).strong().weak());
        ui.add_space(4.0);
        ui.weak("Press the red button, then join or continue your meeting.");
        ui.weak("Your microphone shows as Me; everything the Mac plays shows as Others.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(speaker: Speaker, text: &str) -> Line {
        Line {
            speaker,
            start: 0.0,
            end: 0.0,
            text: text.into(),
            language: None,
        }
    }

    #[test]
    fn matches_follow_the_speaker_filter() {
        let lines = [
            line(Speaker::Me, "Ship it on Friday"),
            line(Speaker::Others, "Friday? ship it, ship it"),
        ];
        let all = transcript_matches(&lines, None, "ship");
        let at: Vec<_> = all.iter().map(|m| (m.line, m.range.clone())).collect();
        assert_eq!(at, [(0, 0..4), (1, 8..12), (1, 17..21)]);
        assert_eq!(transcript_matches(&lines, Some(Speaker::Me), "ship").len(), 1);
        assert!(transcript_matches(&lines, None, "").is_empty());
    }

    #[test]
    fn stepping_wraps_around() {
        assert_eq!(step_index(0, 3, true), 1);
        assert_eq!(step_index(2, 3, true), 0);
        assert_eq!(step_index(0, 3, false), 2);
        assert_eq!(step_index(0, 0, true), 0);
        assert_eq!(step_index(0, 0, false), 0);
    }
}
