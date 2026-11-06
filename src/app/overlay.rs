//! The screenshot overlay: the frozen display, full screen in a borderless
//! window of its own above everything, dimmed outside what is chosen. Drag
//! to select (its size in pixels shows), drag the handles to change it or
//! inside to move it; before that, the window under the mouse lights up
//! and a click takes it (or the whole display). A toolbar beside the
//! selection has the drawing tools (`drawing.rs`), and copies (Enter, or a
//! double-click), saves, pins it to the screen, starts a scrolling
//! screenshot or a screen recording of it, or cancels (Esc); right-click
//! puts the tool away, else drops the selection, or cancels when there is
//! none. The thin outline and the small window beside an area being
//! scrolled or recorded are here too (`outline`, `beside`).

use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Key, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
    ViewportBuilder, ViewportCommand, ViewportId, pos2, vec2,
};

use super::drawing::Drawing;
use crate::capture::annotate::Mark;
use crate::capture::image::Image;
use crate::capture::record;
use crate::capture::scrolling::{self, Area};
use crate::capture::select::{self, Handle, Mapping};
use crate::capture::{Shot, hide_from_captures, raise_overlay};

/// The overlay window's title, by which the system's window is found.
const TITLE: &str = "Meeting Transcriber screenshot";
/// Over what is not chosen.
const DIM: Color32 = Color32::from_black_alpha(120);
const OUTLINE: Color32 = Color32::from_rgb(40, 140, 255);
/// Half a handle's side, and how near the pointer must be to take one.
const HANDLE: f32 = 4.0;
const REACH: f32 = 8.0;
/// A selection smaller than this (in points) was meant as a click.
const MIN_SIZE: f32 = 4.0;
/// Between the selection and its toolbar.
const GAP: f32 = 8.0;
/// For how many frames a window's place is checked and set right.
pub(super) const PLACE_FRAMES: u32 = 30;
/// The thickness of the outline around an area being scrolled or
/// recorded, in points.
const THICKNESS: f32 = 2.0;

/// What was chosen: the pixels, and what was drawn on them (in their
/// pixels), to be drawn onto them.
pub(super) struct Picture {
    pub image: Image,
    pub marks: Vec<Mark>,
}

/// How the overlay was closed.
pub(super) enum Finish {
    /// Put on the clipboard, and saved.
    Copy(Picture),
    Save(Picture),
    /// Shown above the other windows, where it was on the desktop: its
    /// top-left corner in the desktop's pixels, and the display's scale.
    Pin {
        picture: Picture,
        at: [i32; 2],
        scale: f32,
    },
    /// A scrolling screenshot of the area, from now on.
    Scroll(Area),
    /// A screen recording of the area, from now on.
    Record(Area),
    Cancel,
}

/// A button on the toolbar beside the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Cancel,
    Scroll,
    Record,
    Pin,
    Save,
    Copy,
}

impl Tool {
    /// Left to right, after the drawing tools.
    const TOOLBAR: [Tool; 6] = [
        Tool::Cancel,
        Tool::Scroll,
        Tool::Record,
        Tool::Pin,
        Tool::Save,
        Tool::Copy,
    ];

    fn label(self) -> &'static str {
        match self {
            Tool::Cancel => "Cancel",
            Tool::Scroll => "Scroll",
            Tool::Record => "Record",
            Tool::Pin => "Pin",
            Tool::Save => "Save",
            Tool::Copy => "Copy",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tool::Cancel => "Esc",
            Tool::Scroll => "A long screenshot: the area scrolls down and its pictures are joined",
            Tool::Record => {
                "A video of the area, with sound, until you stop it (Stop beside it, or the screenshot hotkey)"
            }
            Tool::Pin => "Keep the picture on the screen, above other windows (not saved)",
            Tool::Save if cfg!(target_os = "macos") => "Save the picture, not on the clipboard (⌘S)",
            Tool::Save => "Save the picture, not on the clipboard (Ctrl+S)",
            Tool::Copy => "Copy to the clipboard and save (Enter, or double-click)",
        }
    }
}

/// Why Scroll and Record are greyed out, when they are.
struct Blocked {
    scroll: Option<&'static str>,
    record: Option<&'static str>,
}

/// What the pointer is doing with the selection.
#[derive(Debug, Clone, Copy)]
enum Drag {
    Idle,
    /// Drawing a new selection from here.
    Selecting(Pos2),
    /// Dragging a handle of the selection as it was at the start.
    Adjusting {
        handle: Handle,
        start: Rect,
        from: Pos2,
    },
    /// Drawing or moving a mark.
    Drawing,
}

pub(super) struct Overlay {
    shot: Shot,
    texture: egui::TextureHandle,
    /// In the overlay's points.
    selection: Option<Rect>,
    drag: Drag,
    drawing: Drawing,
    /// The toolbar's size when last shown, to place it.
    toolbar: Vec2,
    frames: u32,
    raised: bool,
}

impl Overlay {
    pub fn new(ctx: &egui::Context, shot: Shot) -> Self {
        // Wider than the graphics card takes (a big display at full
        // resolution): shown a little softer; what is saved is not.
        let max = ctx.input(|i| i.max_texture_side) as u32;
        let shrunk;
        let shown = if shot.image.width.max(shot.image.height) > max {
            shrunk = shot.image.shrunk_to(max);
            &shrunk
        } else {
            &shot.image
        };
        let pixels =
            egui::ColorImage::from_rgba_unmultiplied([shown.width as usize, shown.height as usize], &shown.rgba);
        let texture = ctx.load_texture("screenshot", pixels, egui::TextureOptions::LINEAR);
        Self {
            shot,
            texture,
            selection: None,
            drag: Drag::Idle,
            drawing: Drawing::default(),
            toolbar: vec2(200.0, 36.0),
            frames: 0,
            raised: false,
        }
    }

    pub fn taken(&self) -> chrono::NaiveDateTime {
        self.shot.taken
    }

    /// Shows the overlay's window for this frame; Some once it is done.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Finish> {
        let d = self.shot.display;
        let scale = d.scale.max(0.25);
        let (width, height) = (self.shot.image.width as f32, self.shot.image.height as f32);
        let builder = ViewportBuilder::default()
            .with_title(TITLE)
            .with_position(pos2(d.origin_px[0] as f32 / scale, d.origin_px[1] as f32 / scale))
            .with_inner_size(vec2(width / scale, height / scale))
            .with_decorations(false)
            .with_resizable(false)
            .with_has_shadow(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_active(true);
        ctx.show_viewport_immediate(ViewportId::from_hash_of("screenshot overlay"), builder, |ui, _| {
            self.ui(ui)
        })
    }

    /// Sets the window over the display exactly and raises it.
    fn place(&mut self, ctx: &egui::Context) {
        self.frames += 1;
        if !self.raised {
            self.raised = raise_overlay(TITLE, true);
            if self.raised {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
        }
        if self.frames > PLACE_FRAMES {
            return;
        }
        let d = self.shot.display;
        let origin = pos2(d.origin_px[0] as f32, d.origin_px[1] as f32);
        let size = vec2(self.shot.image.width as f32, self.shot.image.height as f32);
        keep_at(ctx, origin, Some(size));
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut Ui) -> Option<Finish> {
        let ctx = ui.ctx().clone();
        self.place(&ctx);
        let screen = ui.max_rect();
        let map = Mapping {
            screen,
            width: self.shot.image.width,
            height: self.shot.image.height,
        };
        self.drawing.begin(&map);
        let response = ui.allocate_rect(screen, Sense::click_and_drag());
        // While a text is typed, its keys are its own.
        let typing = self.drawing.typing();
        let (escape, closing, copy, save) = ui.input(|i| {
            (
                i.key_pressed(Key::Escape),
                i.viewport().close_requested(),
                !typing && (i.key_pressed(Key::Enter) || i.events.iter().any(|e| matches!(e, egui::Event::Copy))),
                !typing && i.modifiers.command && i.key_pressed(Key::S),
            )
        });
        if escape && typing {
            self.drawing.cancel_text();
        } else if escape || closing {
            return Some(Finish::Cancel);
        }
        if self.selection.is_some() {
            self.drawing.shortcuts(ui);
        }
        self.follow_drag(&response, ui, &map);
        let pointer = response.hover_pos();
        // Before anything is chosen, the window under the mouse.
        let hovered = match (self.selection, self.drag, pointer) {
            (None, Drag::Idle, Some(p)) => {
                let px = map.to_pixel(p);
                Some(select::window_at(&self.shot.windows, px.x, px.y).map_or(screen, |w| map.to_points(w)))
            }
            _ => None,
        };
        if response.clicked() {
            match (self.selection, pointer) {
                (None, _) => self.selection = hovered.or(Some(screen)),
                (Some(sel), Some(p)) => self.drawing.click(ui.painter(), map.to_pixel(p), sel.contains(p)),
                (Some(_), None) => {}
            }
        }
        if response.secondary_clicked() {
            if self.drawing.kind.is_some() || self.drawing.typing() {
                self.drawing.set_kind(None, ui.painter());
            } else if self.drawing.marks.is_empty() && self.selection.take().is_none() {
                return Some(Finish::Cancel);
            }
        }
        let mut chosen = None;
        if response.double_clicked()
            && self.drawing.kind.is_none()
            && let Some((sel, p)) = self.selection.zip(pointer)
            && sel.contains(p)
            && !self.drawing.over_mark(map.to_pixel(p))
        {
            chosen = Some(Tool::Copy);
        }
        if self.selection.is_some() {
            if copy {
                chosen = Some(Tool::Copy);
            } else if save {
                chosen = Some(Tool::Save);
            }
        }

        self.paint(ui, screen, &map, hovered);
        if let Some(sel) = self.selection {
            self.drawing.text_editor(ui, &map, sel);
        }
        let blocked = self.blocked(&map);
        if let Some(sel) = self.selection
            && matches!(self.drag, Drag::Idle)
            && let Some(tool) = self.toolbar(&ctx, sel, screen, &blocked)
        {
            chosen = Some(tool);
        }
        if response.hovered() || response.dragged() {
            ctx.set_cursor_icon(self.cursor(pointer, &map));
        }
        let chosen = chosen?;
        if matches!(chosen, Tool::Copy | Tool::Save | Tool::Pin) {
            self.drawing.commit_text(ui.painter());
        }
        let pixels = self.selection.and_then(|s| map.to_pixels(s));
        let picture = |px: select::PxRect| Picture {
            image: self.shot.image.crop(px),
            marks: self.drawing.marks.translated(vec2(-(px.x as f32), -(px.y as f32))),
        };
        let d = self.shot.display;
        match chosen {
            Tool::Cancel => Some(Finish::Cancel),
            Tool::Copy => pixels.map(picture).map(Finish::Copy),
            Tool::Save => pixels.map(picture).map(Finish::Save),
            Tool::Pin => pixels.map(|px| Finish::Pin {
                picture: picture(px),
                at: [d.origin_px[0] + px.x as i32, d.origin_px[1] + px.y as i32],
                scale: d.scale,
            }),
            Tool::Scroll | Tool::Record => pixels.map(|rect| {
                let area = Area {
                    display: d,
                    display_size: [self.shot.image.width, self.shot.image.height],
                    rect,
                };
                if chosen == Tool::Scroll {
                    Finish::Scroll(area)
                } else {
                    Finish::Record(area)
                }
            }),
        }
    }

    /// Why Scroll and Record cannot be used on the selection, if they
    /// cannot: they take new pictures, which have nothing drawn on them,
    /// and need an area big enough.
    fn blocked(&self, map: &Mapping) -> Blocked {
        let px = self.selection.and_then(|s| map.to_pixels(s));
        let drawn = !self.drawing.marks.is_empty();
        let scroll = if drawn {
            Some("Scrolling takes new pictures: only without drawings")
        } else if px.is_none_or(|px| px.height < scrolling::MIN_HEIGHT) {
            Some("Select a taller area to scroll")
        } else {
            None
        };
        let record = if let Some(why) = record::unsupported() {
            Some(why)
        } else if drawn {
            Some("A recording shows the screen as it is: only without drawings")
        } else if px.is_none_or(|px| px.width.min(px.height) < record::MIN_SIDE) {
            Some("Select a bigger area to record")
        } else {
            None
        };
        Blocked { scroll, record }
    }

    /// Draws a new selection, or changes the one there, as the pointer
    /// drags; or with a drawing tool (or on a mark), draws or moves a mark.
    fn follow_drag(&mut self, response: &egui::Response, ui: &Ui, map: &Mapping) {
        let screen = map.screen;
        if response.drag_started() {
            let from = ui
                .input(|i| i.pointer.press_origin())
                .or(response.interact_pointer_pos())
                .unwrap_or(screen.min);
            self.drag = match self.selection {
                Some(_) if self.drawing.takes(map.to_pixel(from)) => {
                    self.drawing.press(ui.painter(), map.to_pixel(from));
                    Drag::Drawing
                }
                Some(s) if let Some(handle) = select::handle_at(s, from, REACH) => {
                    Drag::Adjusting { handle, start: s, from }
                }
                _ => Drag::Selecting(from),
            };
        }
        if response.dragged()
            && let Some(p) = response.interact_pointer_pos()
        {
            match self.drag {
                Drag::Selecting(from) => self.selection = Some(select::span(from, p, screen)),
                Drag::Adjusting { handle, start, from } => {
                    self.selection = Some(select::drag(start, handle, p - from, screen));
                }
                Drag::Drawing => {
                    if let Some(sel) = self.selection {
                        let bounds = Rect::from_min_max(map.to_pixel(sel.min), map.to_pixel(sel.max));
                        self.drawing.drag_to(map.to_pixel(p), bounds);
                    }
                }
                Drag::Idle => {}
            }
        }
        if response.drag_stopped() {
            if matches!(self.drag, Drag::Drawing) {
                self.drawing.release();
            } else if self
                .selection
                .is_some_and(|s| s.width() < MIN_SIZE || s.height() < MIN_SIZE)
            {
                self.selection = None;
            }
            self.drag = Drag::Idle;
        }
    }

    fn paint(&mut self, ui: &Ui, screen: Rect, map: &Mapping, hovered: Option<Rect>) {
        let painter = ui.painter();
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        painter.image(self.texture.id(), screen, uv, Color32::WHITE);
        if let Some(sel) = self.selection {
            self.drawing.paint(&painter.with_clip_rect(sel), map, &self.shot.image);
        }
        let lit = self.selection.or(hovered);
        match lit {
            Some(r) => {
                // Four bands around what is lit.
                for band in [
                    Rect::from_min_max(screen.min, pos2(screen.max.x, r.min.y)),
                    Rect::from_min_max(pos2(screen.min.x, r.max.y), screen.max),
                    Rect::from_min_max(pos2(screen.min.x, r.min.y), pos2(r.min.x, r.max.y)),
                    Rect::from_min_max(pos2(r.max.x, r.min.y), pos2(screen.max.x, r.max.y)),
                ] {
                    if band.is_positive() {
                        painter.rect_filled(band, 0.0, DIM);
                    }
                }
                let width = if self.selection.is_some() { 1.5 } else { 3.0 };
                painter.rect_stroke(r, 0.0, Stroke::new(width, OUTLINE), StrokeKind::Inside);
            }
            None => {
                painter.rect_filled(screen, 0.0, DIM);
            }
        }
        let Some(sel) = self.selection else {
            label(
                ui,
                pos2(screen.center().x, screen.top() + 40.0),
                Align2::CENTER_TOP,
                "Drag to select  ·  click a window  ·  Esc cancels",
            );
            return;
        };
        // With a drawing tool, drags draw: the handles are put away.
        if self.drawing.kind.is_none() {
            for handle in Handle::DRAWN {
                let square = Rect::from_center_size(handle.position(sel), Vec2::splat(2.0 * HANDLE));
                painter.rect_filled(square, 1.0, Color32::WHITE);
                painter.rect_stroke(square, 1.0, Stroke::new(1.0, OUTLINE), StrokeKind::Inside);
            }
        }
        if let Some(px) = map.to_pixels(sel) {
            let text = format!("{} × {}", px.width, px.height);
            let (at, anchor) = if sel.top() - screen.top() > 26.0 {
                (sel.left_top() - vec2(0.0, 6.0), Align2::LEFT_BOTTOM)
            } else {
                (sel.left_top() + vec2(6.0, 6.0), Align2::LEFT_TOP)
            };
            label(ui, at, anchor, &text);
        }
    }

    /// The toolbar beside the selection: the drawing tools, then what to do
    /// with the picture, and the colours and sizes while a tool is in use.
    /// The tool clicked, if one was. Scroll and Record are greyed out when
    /// `blocked` says why.
    fn toolbar(&mut self, ctx: &egui::Context, sel: Rect, screen: Rect, blocked: &Blocked) -> Option<Tool> {
        let at = select::toolbar_origin(sel, self.toolbar, screen, GAP);
        let mut chosen = None;
        let shown = egui::Area::new(egui::Id::unique("screenshot toolbar"))
            .order(Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        self.drawing.tool_buttons(ui);
                        ui.separator();
                        for tool in Tool::TOOLBAR {
                            let why = match tool {
                                Tool::Scroll => blocked.scroll,
                                Tool::Record => blocked.record,
                                _ => None,
                            };
                            if ui
                                .add_enabled(why.is_none(), egui::Button::new(tool.label()))
                                .on_hover_text(tool.hint())
                                .on_disabled_hover_text(why.unwrap_or_default())
                                .clicked()
                            {
                                chosen = Some(tool);
                            }
                        }
                    });
                    if self.drawing.kind.is_some() {
                        ui.horizontal(|ui| self.drawing.style_buttons(ui));
                    }
                })
            });
        self.toolbar = shown.response.rect.size();
        chosen
    }

    /// The pointer for what a drag would do here.
    fn cursor(&self, pointer: Option<Pos2>, map: &Mapping) -> CursorIcon {
        let drawing = match self.drag {
            Drag::Idle | Drag::Drawing if self.selection.is_some() => {
                self.drawing.cursor(pointer.map(|p| map.to_pixel(p)))
            }
            _ => None,
        };
        if let Some(icon) = drawing {
            return icon;
        }
        let handle = match self.drag {
            Drag::Adjusting { handle, .. } => Some(handle),
            Drag::Selecting(_) | Drag::Drawing => None,
            Drag::Idle => self
                .selection
                .zip(pointer)
                .and_then(|(s, p)| select::handle_at(s, p, REACH)),
        };
        match handle {
            None => CursorIcon::Crosshair,
            Some(Handle::Move) => CursorIcon::Move,
            Some(Handle::Left | Handle::Right) => CursorIcon::ResizeHorizontal,
            Some(Handle::Top | Handle::Bottom) => CursorIcon::ResizeVertical,
            Some(Handle::TopLeft | Handle::BottomRight) => CursorIcon::ResizeNwSe,
            Some(Handle::TopRight | Handle::BottomLeft) => CursorIcon::ResizeNeSw,
        }
    }
}

/// Moves the viewport being drawn to `origin` on the desktop (in its
/// pixels), and with `size` sizes it, when it is more than a point off: the
/// first guess misses where displays scale differently.
pub(super) fn keep_at(ctx: &egui::Context, origin: Pos2, size: Option<Vec2>) {
    let ppp = ctx.pixels_per_point();
    let Some(have) = ctx.input(|i| i.viewport().inner_rect) else {
        return;
    };
    let min = (origin.to_vec2() / ppp).to_pos2();
    if have.min.distance(min) > 1.0 {
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(min));
    }
    if let Some(size) = size.map(|s| s / ppp)
        && (have.size() - size).length() > 1.0
    {
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
    }
}

/// A thin outline just outside `area`, which stays out of its pictures:
/// four click-through bars, windows of their own titled `title` and a
/// number. `ready` says, for each, whether it is raised and left out of
/// screen captures yet; `placing`, whether their places are still checked.
pub(super) fn outline(ctx: &egui::Context, area: &Area, title: &str, ready: &mut [bool], placing: bool) {
    let scale = area.display.scale.max(0.25);
    let thickness = (THICKNESS * scale).round().max(1.0) as i32;
    for (i, ([x, y, width, height], ready)) in area.outline(thickness).into_iter().zip(ready).enumerate() {
        let title = format!("{title} {}", i + 1);
        let (origin, size) = (pos2(x as f32, y as f32), vec2(width as f32, height as f32));
        let builder = ViewportBuilder::default()
            .with_title(&title)
            .with_position((origin.to_vec2() / scale).to_pos2())
            .with_inner_size(size / scale)
            .with_decorations(false)
            .with_resizable(false)
            .with_has_shadow(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_mouse_passthrough(true)
            .with_active(false);
        ctx.show_viewport_immediate(ViewportId::from_hash_of(&title), builder, |ui, _| {
            if !*ready {
                *ready = raise_overlay(&title, false) && hide_from_captures(&title);
            }
            if placing {
                keep_at(ui.ctx(), origin, Some(size));
            }
            ui.painter().rect_filled(ui.max_rect(), 0.0, OUTLINE);
        });
    }
}

/// Where a window of `size` beside `area` goes on the desktop, in its
/// pixels (`size` and `gap` too): under the area, else above it, else
/// inside it at the bottom right; on its display.
pub(super) fn beside(area: &Area, size: Vec2, gap: f32) -> Pos2 {
    let r = area.rect;
    let rect = Rect::from_min_size(pos2(r.x as f32, r.y as f32), vec2(r.width as f32, r.height as f32));
    let [width, height] = area.display_size;
    let display = Rect::from_min_size(Pos2::ZERO, vec2(width as f32, height as f32));
    let [x, y] = area.display.origin_px;
    select::toolbar_origin(rect, size, display, gap) + vec2(x as f32, y as f32)
}

/// White text on a dark rounded tag.
fn label(ui: &Ui, at: Pos2, anchor: Align2, text: &str) {
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(13.0), Color32::WHITE);
    let rect = anchor.anchor_size(at, galley.size() + vec2(12.0, 6.0));
    painter.rect_filled(rect, 4.0, Color32::from_black_alpha(190));
    painter.galley(rect.min + vec2(6.0, 3.0), galley, Color32::WHITE);
}
