//! Drawing on the screenshot overlay: the tools (rectangle, ellipse, arrow,
//! pen, text and mosaic) with their colour and size, drawing and moving
//! marks with the pointer, typing a text, undo and redo, and showing the
//! marks as they will be kept. The marks and their geometry are
//! `capture::annotate`; `capture::raster` draws them onto the picture.

use std::collections::HashMap;

use eframe::egui::{
    self, Color32, CursorIcon, FontId, Key, Modifiers, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    Vec2, pos2, vec2,
};

use crate::capture::annotate::{self, Mark, Marks, Rgba, Shape};
use crate::capture::image::Image;
use crate::capture::select::{Mapping, PxRect};

/// A drawing tool on the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Rectangle,
    Ellipse,
    Arrow,
    Pen,
    Text,
    Mosaic,
}

impl Kind {
    /// Left to right.
    const ALL: [Kind; 6] = [
        Kind::Rectangle,
        Kind::Ellipse,
        Kind::Arrow,
        Kind::Pen,
        Kind::Text,
        Kind::Mosaic,
    ];

    fn hint(self) -> &'static str {
        match self {
            Kind::Rectangle => "Rectangle (R)",
            Kind::Ellipse => "Ellipse (E)",
            Kind::Arrow => "Arrow (A)",
            Kind::Pen => "Pen (P)",
            Kind::Text => "Text (T): click where it goes, type, then Enter",
            Kind::Mosaic => "Mosaic (M): drag over what to hide",
        }
    }

    fn key(self) -> Key {
        match self {
            Kind::Rectangle => Key::R,
            Kind::Ellipse => Key::E,
            Kind::Arrow => Key::A,
            Kind::Pen => Key::P,
            Kind::Text => Key::T,
            Kind::Mosaic => Key::M,
        }
    }
}

/// The colours to draw in.
const PALETTE: [Rgba; 6] = [
    [235, 50, 50, 255],
    [255, 200, 0, 255],
    [40, 190, 80, 255],
    [40, 140, 255, 255],
    [255, 255, 255, 255],
    [20, 20, 20, 255],
];
/// Small, medium and large, in points: lines' widths, text's sizes and
/// mosaic blocks' sides.
const WIDTHS: [f32; 3] = [2.0, 4.0, 8.0];
const TEXT_SIZES: [f32; 3] = [16.0, 24.0, 36.0];
const BLOCKS: [f32; 3] = [6.0, 10.0, 16.0];
/// How near the pointer must be to a mark to take it, in points.
const REACH: f32 = 5.0;
/// A toolbar button's side, in points.
const BUTTON: f32 = 28.0;

/// What a drag is doing to the marks.
enum Doing {
    Nothing,
    /// A new mark from `start`, kept when the drag ends.
    Drawing {
        mark: Mark,
        start: Pos2,
    },
    /// The mark at `index`, as it was before, dragged from `from`.
    Moving {
        index: usize,
        before: Mark,
        from: Pos2,
    },
}

/// A text being typed, at a point in the picture's pixels.
struct Typing {
    at: Pos2,
    text: String,
    color: Rgba,
    /// In points.
    size: f32,
    focused: bool,
}

/// The marks on the overlay and the tools that make them.
pub(super) struct Drawing {
    pub marks: Marks,
    /// The tool in use; without one, drags move marks or the selection.
    pub kind: Option<Kind>,
    color: usize,
    size: usize,
    doing: Doing,
    typing: Option<Typing>,
    /// The mark last drawn or clicked, which Delete removes.
    picked: Option<usize>,
    /// The overlay's pixels per point this frame.
    ppp: f32,
    /// Each mosaic's blocks, by where it is and their size.
    mosaics: HashMap<[u32; 5], Vec<(PxRect, Rgba)>>,
}

impl Default for Drawing {
    fn default() -> Self {
        Self {
            marks: Marks::default(),
            kind: None,
            color: 0,
            size: 1,
            doing: Doing::Nothing,
            typing: None,
            picked: None,
            ppp: 1.0,
            mosaics: HashMap::new(),
        }
    }
}

impl Drawing {
    /// Called first each frame, with how the overlay maps to pixels.
    pub fn begin(&mut self, map: &Mapping) {
        self.ppp = map.pixels_per_point().max(0.25);
    }

    /// A text is being typed: keys go to it.
    pub fn typing(&self) -> bool {
        self.typing.is_some()
    }

    fn reach(&self) -> f32 {
        REACH * self.ppp
    }

    /// Whether a drag from `p` (in pixels) draws or moves a mark rather
    /// than the selection.
    pub fn takes(&self, p: Pos2) -> bool {
        self.kind.is_some() || self.typing.is_some() || self.marks.at(p, self.reach()).is_some()
    }

    /// A drag starts at `p`: moves the mark there, or starts a new one.
    pub fn press(&mut self, painter: &Painter, p: Pos2) {
        self.commit_text(painter);
        if let Some(index) = self.marks.at(p, self.reach()) {
            self.picked = Some(index);
            self.doing = Doing::Moving {
                index,
                before: self.marks.list()[index].clone(),
                from: p,
            };
            return;
        }
        let Some(kind) = self.kind else {
            return;
        };
        if kind == Kind::Text {
            self.start_text(p);
            return;
        }
        let shape = match kind {
            Kind::Rectangle => Shape::Rectangle(Rect::from_two_pos(p, p)),
            Kind::Ellipse => Shape::Ellipse(Rect::from_two_pos(p, p)),
            Kind::Mosaic => Shape::Mosaic(annotate::pixel_rect(p, p)),
            Kind::Arrow => Shape::Arrow { from: p, to: p },
            Kind::Pen | Kind::Text => Shape::Pen(vec![p]),
        };
        let size = match kind {
            Kind::Mosaic => BLOCKS[self.size],
            _ => WIDTHS[self.size],
        } * self.ppp;
        let mark = Mark {
            shape,
            color: PALETTE[self.color],
            size,
        };
        self.doing = Doing::Drawing { mark, start: p };
    }

    /// The pointer dragged on to `p`, kept inside `bounds` (the selection,
    /// in pixels).
    pub fn drag_to(&mut self, p: Pos2, bounds: Rect) {
        let p = bounds.clamp(p);
        match &mut self.doing {
            Doing::Nothing => {}
            Doing::Drawing { mark, start } => match &mut mark.shape {
                Shape::Rectangle(r) | Shape::Ellipse(r) => *r = Rect::from_two_pos(*start, p),
                Shape::Mosaic(r) => *r = annotate::pixel_rect(*start, p),
                Shape::Arrow { to, .. } => *to = p,
                Shape::Pen(points) => {
                    if points.last().is_none_or(|last| last.distance(p) >= 0.75 * self.ppp) {
                        points.push(p);
                    }
                }
                Shape::Text { .. } => {}
            },
            Doing::Moving { index, before, from } => {
                let moved = before.translated(p - *from);
                self.marks.set(*index, moved);
            }
        }
    }

    /// The drag ended: a new mark is kept, a moved one can be undone.
    pub fn release(&mut self) {
        match std::mem::replace(&mut self.doing, Doing::Nothing) {
            Doing::Nothing => {}
            Doing::Drawing { mark, .. } => {
                if !mark.is_empty() {
                    self.marks.add(mark);
                    self.picked = Some(self.marks.list().len() - 1);
                }
            }
            Doing::Moving { index, before, .. } => self.marks.edited(index, before),
        }
    }

    /// A click at `p` (pixels), `inside` the selection: ends a text being
    /// typed, else picks the mark there, else starts a text there.
    pub fn click(&mut self, painter: &Painter, p: Pos2, inside: bool) {
        if self.typing.is_some() {
            self.commit_text(painter);
            return;
        }
        self.picked = self.marks.at(p, self.reach());
        if self.picked.is_none() && inside && self.kind == Some(Kind::Text) {
            self.start_text(p);
        }
    }

    /// Whether a mark is under `p` (pixels).
    pub fn over_mark(&self, p: Pos2) -> bool {
        self.marks.at(p, self.reach()).is_some()
    }

    fn start_text(&mut self, at: Pos2) {
        self.picked = None;
        self.typing = Some(Typing {
            at,
            text: String::new(),
            color: PALETTE[self.color],
            size: TEXT_SIZES[self.size],
            focused: false,
        });
    }

    /// Keeps the text being typed as a mark, if anything was typed.
    pub fn commit_text(&mut self, painter: &Painter) {
        let Some(typing) = self.typing.take() else {
            return;
        };
        let text = typing.text.replace(['\n', '\r'], " ");
        let font = FontId::proportional(typing.size);
        let extent = painter.layout_no_wrap(text.clone(), font, Color32::WHITE).size() * self.ppp;
        let mark = Mark {
            shape: Shape::Text {
                at: typing.at,
                text,
                extent,
            },
            color: typing.color,
            size: typing.size * self.ppp,
        };
        if !mark.is_empty() {
            self.marks.add(mark);
            self.picked = Some(self.marks.list().len() - 1);
        }
    }

    /// Drops the text being typed (Esc).
    pub fn cancel_text(&mut self) {
        self.typing = None;
    }

    /// Picks a tool, or none; the text being typed is kept first.
    pub fn set_kind(&mut self, kind: Option<Kind>, painter: &Painter) {
        self.commit_text(painter);
        self.kind = kind;
    }

    fn undo(&mut self, painter: &Painter) {
        self.commit_text(painter);
        self.picked = None;
        self.marks.undo();
    }

    fn redo(&mut self, painter: &Painter) {
        self.commit_text(painter);
        self.picked = None;
        self.marks.redo();
    }

    /// The tools' keys, undo (⌘Z or Ctrl+Z), redo (⇧⌘Z, Ctrl+Shift+Z or
    /// Ctrl+Y), and Delete for the picked mark; not while typing.
    pub fn shortcuts(&mut self, ui: &Ui) {
        if self.typing.is_some() {
            return;
        }
        let painter = ui.painter();
        let (redo, undo, delete, tool) = ui.input_mut(|i| {
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || (!cfg!(target_os = "macos") && i.consume_key(Modifiers::CTRL, Key::Y));
            let undo = i.consume_key(Modifiers::COMMAND, Key::Z);
            let delete = i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace);
            let tool = Kind::ALL.into_iter().find(|k| i.consume_key(Modifiers::NONE, k.key()));
            (redo, undo, delete, tool)
        });
        if redo {
            self.redo(painter);
        } else if undo {
            self.undo(painter);
        }
        if delete && let Some(index) = self.picked.take() {
            self.marks.remove(index);
        }
        if let Some(kind) = tool {
            self.kind = (self.kind != Some(kind)).then_some(kind);
        }
    }

    /// The pointer for what pressing at `p` (pixels) would do; None when
    /// it is the selection's business.
    pub fn cursor(&self, p: Option<Pos2>) -> Option<CursorIcon> {
        match self.doing {
            Doing::Moving { .. } => return Some(CursorIcon::Grabbing),
            Doing::Drawing { .. } => return Some(CursorIcon::Crosshair),
            Doing::Nothing => {}
        }
        if p.is_some_and(|p| self.over_mark(p)) {
            return Some(CursorIcon::Grab);
        }
        match self.kind? {
            Kind::Text => Some(CursorIcon::Text),
            _ => Some(CursorIcon::Crosshair),
        }
    }

    /// Paints the marks, the one being drawn, and an outline around the
    /// picked one. `painter` is clipped to the selection; `image` is the
    /// screenshot, for the mosaics.
    pub fn paint(&mut self, painter: &Painter, map: &Mapping, image: &Image) {
        let mut used = Vec::new();
        let drawing = match &self.doing {
            Doing::Drawing { mark, .. } => Some(mark),
            _ => None,
        };
        for mark in self.marks.list().iter().chain(drawing) {
            if let Shape::Mosaic(rect) = &mark.shape {
                let key = [
                    rect.min.x.to_bits(),
                    rect.min.y.to_bits(),
                    rect.max.x.to_bits(),
                    rect.max.y.to_bits(),
                    mark.size.to_bits(),
                ];
                let blocks = self
                    .mosaics
                    .entry(key)
                    .or_insert_with(|| annotate::mosaic_blocks(image, *rect, mark.size));
                for (cell, color) in blocks.iter() {
                    painter.rect_filled(map.to_points(*cell), 0.0, color32(*color));
                }
                used.push(key);
            } else {
                paint_mark(painter, map, mark, self.ppp);
            }
        }
        self.mosaics.retain(|key, _| used.contains(key));
        if let Some(mark) = self.picked.and_then(|i| self.marks.list().get(i)) {
            let bounds = mark.bounds();
            let r = Rect::from_min_max(map.to_point(bounds.min), map.to_point(bounds.max)).expand(3.0);
            let corners = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
                r.left_top(),
            ];
            painter.extend(egui::Shape::dashed_line(
                &corners,
                Stroke::new(1.0, Color32::WHITE),
                4.0,
                3.0,
            ));
        }
    }

    /// The box the text is typed in, while one is. Enter or a click
    /// elsewhere keeps it.
    pub fn text_editor(&mut self, ui: &Ui, map: &Mapping, selection: Rect) {
        let Some(typing) = &mut self.typing else {
            return;
        };
        let at = map.to_point(typing.at);
        let width = (selection.right() - at.x).max(typing.size);
        let shown = egui::Area::new(egui::Id::unique("screenshot text"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::TextEdit::singleline(&mut typing.text)
                    .font(FontId::proportional(typing.size))
                    .text_color(color32(typing.color))
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .desired_width(width)
                    .clip_text(false)
                    .show(ui)
                    .response
            });
        let response = shown.inner;
        if !typing.focused {
            response.request_focus();
            typing.focused = true;
        }
        let r = response.rect.expand(3.0);
        let corners = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        ui.ctx()
            .layer_painter(shown.response.layer_id)
            .extend(egui::Shape::dashed_line(
                &corners,
                Stroke::new(1.0, Color32::WHITE),
                4.0,
                3.0,
            ));
        if response.lost_focus() {
            self.commit_text(ui.painter());
        }
    }

    /// The tools, then undo and redo, on the toolbar.
    pub fn tool_buttons(&mut self, ui: &mut Ui) {
        for kind in Kind::ALL {
            let chosen = self.kind == Some(kind);
            if icon_button(ui, Icon::Tool(kind), chosen, true)
                .on_hover_text(kind.hint())
                .clicked()
            {
                self.set_kind((!chosen).then_some(kind), &ui.painter().clone());
            }
        }
        let (undo, redo) = if cfg!(target_os = "macos") {
            ("Undo (⌘Z)", "Redo (⇧⌘Z)")
        } else {
            ("Undo (Ctrl+Z)", "Redo (Ctrl+Y)")
        };
        let painter = ui.painter().clone();
        if icon_button(ui, Icon::Undo, false, self.marks.can_undo() || self.typing.is_some())
            .on_hover_text(undo)
            .clicked()
        {
            self.undo(&painter);
        }
        if icon_button(ui, Icon::Redo, false, self.marks.can_redo())
            .on_hover_text(redo)
            .clicked()
        {
            self.redo(&painter);
        }
    }

    /// The colours (not for mosaic) and sizes, for the tool in use.
    pub fn style_buttons(&mut self, ui: &mut Ui) {
        if self.kind != Some(Kind::Mosaic) {
            for (i, color) in PALETTE.into_iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON - 6.0), Sense::click());
                let painter = ui.painter();
                if i == self.color {
                    painter.circle_stroke(
                        rect.center(),
                        9.5,
                        Stroke::new(2.0, ui.visuals().selection.stroke.color),
                    );
                }
                painter.circle_filled(rect.center(), 7.0, color32(color));
                painter.circle_stroke(rect.center(), 7.0, Stroke::new(1.0, Color32::from_gray(110)));
                if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    self.color = i;
                    if let Some(typing) = &mut self.typing {
                        typing.color = color;
                    }
                }
            }
            ui.separator();
        }
        for (i, dot) in [2.5f32, 4.0, 6.0].into_iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON - 6.0), Sense::click());
            let visuals = ui.visuals();
            if i == self.size {
                ui.painter().rect_filled(rect, 4.0, visuals.selection.bg_fill);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 4.0, visuals.widgets.hovered.bg_fill);
            }
            ui.painter().circle_filled(rect.center(), dot, visuals.text_color());
            let what = match (self.kind, i) {
                (Some(Kind::Text), 0) => "Small text",
                (Some(Kind::Text), 1) => "Medium text",
                (Some(Kind::Text), _) => "Large text",
                (Some(Kind::Mosaic), 0) => "Small blocks",
                (Some(Kind::Mosaic), 1) => "Medium blocks",
                (Some(Kind::Mosaic), _) => "Large blocks",
                (_, 0) => "Thin",
                (_, 1) => "Medium",
                _ => "Thick",
            };
            if response.on_hover_text(what).clicked() {
                self.size = i;
                if let Some(typing) = &mut self.typing {
                    typing.size = TEXT_SIZES[i];
                }
            }
        }
    }
}

fn color32([r, g, b, a]: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// One mark (not a mosaic) on the overlay, as `capture::raster` draws it.
fn paint_mark(painter: &Painter, map: &Mapping, mark: &Mark, ppp: f32) {
    let color = color32(mark.color);
    let stroke = Stroke::new(mark.size / ppp, color);
    let rect = |r: &Rect| Rect::from_min_max(map.to_point(r.min), map.to_point(r.max));
    match &mark.shape {
        Shape::Rectangle(r) => {
            painter.rect_stroke(rect(r), 0.0, stroke, StrokeKind::Middle);
        }
        Shape::Ellipse(r) => {
            let r = rect(r);
            painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
        }
        Shape::Arrow { from, to } => {
            if let Some(head) = annotate::arrow_head(*from, *to, mark.size) {
                let start = map.to_point(*from);
                painter.line_segment([start, map.to_point(head.shaft_end)], stroke);
                painter.circle_filled(start, stroke.width / 2.0, color);
                let points = head.points.iter().map(|p| map.to_point(*p)).collect();
                painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
            }
        }
        Shape::Pen(points) => {
            let points: Vec<Pos2> = points.iter().map(|p| map.to_point(*p)).collect();
            if let [only] = points.as_slice() {
                painter.circle_filled(*only, stroke.width / 2.0, color);
            } else {
                for end in [points.first(), points.last()].into_iter().flatten() {
                    painter.circle_filled(*end, stroke.width / 2.0, color);
                }
                painter.add(egui::Shape::line(points, stroke));
            }
        }
        Shape::Text { at, text, .. } => {
            painter.text(
                map.to_point(*at),
                egui::Align2::LEFT_TOP,
                text,
                FontId::proportional(mark.size / ppp),
                color,
            );
        }
        Shape::Mosaic(_) => {}
    }
}

/// What a toolbar button shows.
#[derive(Clone, Copy)]
enum Icon {
    Tool(Kind),
    Undo,
    Redo,
}

/// A square toolbar button with a drawn icon; `chosen` shows it pressed.
fn icon_button(ui: &mut Ui, icon: Icon, chosen: bool, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON), sense);
    let visuals = ui.visuals();
    if chosen {
        ui.painter().rect_filled(rect, 4.0, visuals.selection.bg_fill);
    } else if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 4.0, visuals.widgets.hovered.bg_fill);
    }
    let color = if !enabled {
        visuals.weak_text_color()
    } else if chosen {
        visuals.selection.stroke.color
    } else {
        visuals.text_color()
    };
    paint_icon(ui.painter(), rect.shrink(7.0), icon, color);
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}

fn paint_icon(painter: &Painter, r: Rect, icon: Icon, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    match icon {
        Icon::Tool(Kind::Rectangle) => {
            painter.rect_stroke(r.shrink2(vec2(0.0, 2.0)), 1.0, stroke, StrokeKind::Middle);
        }
        Icon::Tool(Kind::Ellipse) => {
            painter.add(egui::Shape::ellipse_stroke(
                r.center(),
                vec2(r.width(), r.height() - 4.0) / 2.0,
                stroke,
            ));
        }
        Icon::Tool(Kind::Arrow) => {
            let (from, to) = (r.left_bottom(), r.right_top());
            if let Some(head) = annotate::arrow_head(from, to, 1.0) {
                painter.line_segment([from, head.shaft_end], stroke);
                painter.add(egui::Shape::convex_polygon(head.points.to_vec(), color, Stroke::NONE));
            }
        }
        Icon::Tool(Kind::Pen) => {
            let points = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    pos2(
                        r.left() + t * r.width(),
                        r.center().y + (t * std::f32::consts::TAU).sin() * r.height() * 0.3,
                    )
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
        }
        Icon::Tool(Kind::Text) => {
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                "T",
                FontId::proportional(17.0),
                color,
            );
        }
        Icon::Tool(Kind::Mosaic) => {
            let cell = r.width() / 3.0;
            for (x, y) in (0..3).flat_map(|y| (0..3).map(move |x| (x, y))) {
                let square = Rect::from_min_size(r.min + vec2(x as f32, y as f32) * cell, Vec2::splat(cell));
                let shade = if (x + y) % 2 == 0 {
                    color
                } else {
                    color.gamma_multiply(0.35)
                };
                painter.rect_filled(square, 0.0, shade);
            }
        }
        Icon::Undo | Icon::Redo => {
            // Half a circle over the top, with a head at its end.
            let back = matches!(icon, Icon::Undo);
            let (c, radius) = (r.center() + vec2(0.0, 2.0), r.width() * 0.4);
            let points: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let angle = std::f32::consts::PI * i as f32 / 10.0;
                    let x = if back { angle.cos() } else { -angle.cos() };
                    c + vec2(x, -angle.sin()) * radius
                })
                .collect();
            let end = points[points.len() - 1];
            painter.add(egui::Shape::line(points, stroke));
            let head = [end + vec2(0.0, 4.0), end + vec2(-3.5, -1.0), end + vec2(3.5, -1.0)];
            painter.add(egui::Shape::convex_polygon(head.to_vec(), color, Stroke::NONE));
        }
    }
}
