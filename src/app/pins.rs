//! Pictures pinned to the screen: each in a small borderless window of its
//! own above other windows, at its size in points, so it stays in view
//! while you work. Drag it to move it, scroll or pinch to zoom, double-click
//! or Esc to close it; its right-click menu copies it, saves it like a
//! screenshot (or opens its file), sets it back to its size, or closes it.
//! A screenshot is pinned from the overlay's Pin; a picture in the notes
//! from its menu.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{
    self, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, ViewportBuilder,
    ViewportCommand, ViewportId, pos2, vec2,
};

use super::capture::When;
use super::overlay::keep_at;
use super::{App, platform};
use crate::capture::image::Image;

/// The pins' window titles start with this; a number follows.
const TITLE: &str = "Meeting Transcriber pin";
const BORDER: Color32 = Color32::from_rgb(40, 140, 255);
/// The most a pin zooms in, and the shortest side it zooms out to, in points.
const MAX_ZOOM: f32 = 8.0;
const MIN_SIDE: f32 = 24.0;
/// For how many frames a pinned screenshot's place is checked and set right.
const PLACE_FRAMES: u32 = 30;
/// For how long the zoom shows after it changes.
const ZOOM_SHOWN: Duration = Duration::from_millis(1200);

/// Where a pinned picture came from, which says what saving it does.
pub(super) enum Source {
    /// A screenshot not saved yet: Save keeps it like one.
    Screenshot { when: When, saved: bool },
    /// A picture already in a file.
    File(PathBuf),
}

/// What a pin's window asked for.
enum Action {
    Close,
    Copy,
    Save,
    Open,
}

struct Pin {
    id: u64,
    image: Image,
    texture: egui::TextureHandle,
    /// At 100%, in points.
    size: Vec2,
    zoom: f32,
    /// Where it first shows on the desktop, in points.
    at: Pos2,
    /// Where it belongs on the desktop in pixels, and the pixels per point
    /// of the display there, while it is put right.
    place: Option<(Pos2, f32)>,
    frames: u32,
    source: Source,
    zoomed: Option<Instant>,
}

/// A picture read for pinning, on its thread.
type Loaded = (PathBuf, Result<Image>);

pub(super) struct Pins {
    list: Vec<Pin>,
    next: u64,
    loaded: (Sender<Loaded>, Receiver<Loaded>),
}

impl Default for Pins {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            next: 1,
            loaded: crossbeam_channel::unbounded(),
        }
    }
}

impl Pins {
    /// Adds a pin of `image`, `size` points at 100%, first shown at `at`.
    fn add(
        &mut self,
        ctx: &egui::Context,
        image: Image,
        size: Vec2,
        at: Pos2,
        place: Option<(Pos2, f32)>,
        source: Source,
    ) {
        // Larger than the graphics card takes: shown a little softer.
        let max = ctx.input(|i| i.max_texture_side) as u32;
        let shrunk;
        let shown = if image.width.max(image.height) > max {
            shrunk = image.shrunk_to(max);
            &shrunk
        } else {
            &image
        };
        let pixels =
            egui::ColorImage::from_rgba_unmultiplied([shown.width as usize, shown.height as usize], &shown.rgba);
        let id = self.next;
        self.next += 1;
        let texture = ctx.load_texture(format!("pin {id}"), pixels, egui::TextureOptions::LINEAR);
        self.list.push(Pin {
            id,
            image,
            texture,
            size,
            zoom: 1.0,
            at,
            place,
            frames: 0,
            source,
            zoomed: None,
        });
        ctx.request_repaint();
    }
}

impl App {
    /// Pins a screenshot where it was taken: its top-left corner `at` in
    /// the desktop's pixels, on a display of `scale` pixels per point.
    pub(super) fn pin_screenshot(&mut self, image: Image, at: [i32; 2], scale: f32, when: When) {
        let scale = scale.max(0.25);
        let origin = pos2(at[0] as f32, at[1] as f32);
        let size = vec2(image.width as f32, image.height as f32) / scale;
        let source = Source::Screenshot { when, saved: false };
        let ctx = self.ctx.clone();
        self.capture.pins.add(
            &ctx,
            image,
            size,
            (origin.to_vec2() / scale).to_pos2(),
            Some((origin, scale)),
            source,
        );
    }

    /// Reads the PNG at `path` on a thread, then pins it near the window.
    pub(super) fn pin_file(&mut self, path: PathBuf) {
        let (tx, ctx, file) = (self.capture.pins.loaded.0.clone(), self.ctx.clone(), path.clone());
        let spawned = std::thread::Builder::new().name("pin".into()).spawn(move || {
            let image = std::fs::read(&file)
                .with_context(|| format!("could not read {}", file.display()))
                .and_then(|bytes| Image::from_png(&bytes));
            let _ = tx.send((file, image));
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            self.report(format!("Could not pin {}: {e}", path.display()));
        }
    }

    /// Runs every frame: pins the pictures read, and keeps the pins drawn
    /// while the window is hidden.
    pub(super) fn pins_logic(&mut self, ctx: &egui::Context) {
        let loaded: Vec<Loaded> = self.capture.pins.loaded.1.try_iter().collect();
        for (path, image) in loaded {
            match image {
                Ok(image) => {
                    // Near the window's corner, at the picture's size on its display.
                    let corner = ctx
                        .input(|i| i.viewport().outer_rect)
                        .map_or(pos2(80.0, 80.0), |r| r.min);
                    let size = vec2(image.width as f32, image.height as f32) / ctx.pixels_per_point();
                    let ctx = self.ctx.clone();
                    self.capture
                        .pins
                        .add(&ctx, image, size, corner + vec2(40.0, 60.0), None, Source::File(path));
                }
                Err(e) => self.report(format!("Could not pin {}: {e:#}", path.display())),
            }
        }
        if !self.capture.pins.list.is_empty() {
            // Drawn by the window's frame, which a hidden window would skip.
            ctx.send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden);
        }
    }

    /// Each pin's window. Called from the window's frame.
    pub(super) fn pin_windows(&mut self, ctx: &egui::Context) {
        let mut i = 0;
        while i < self.capture.pins.list.len() {
            let action = self.capture.pins.list[i].show(ctx);
            let pin = &mut self.capture.pins.list[i];
            match action {
                Some(Action::Close) => {
                    self.capture.pins.list.remove(i);
                    continue;
                }
                Some(Action::Copy) => {
                    let result = platform::copy_image(pin.image.width, pin.image.height, &pin.image.rgba);
                    match result {
                        Ok(()) => self.notify("Copied the pinned picture".into(), None),
                        Err(e) => self.report(format!("{e:#}")),
                    }
                }
                Some(Action::Save) => {
                    if let Source::Screenshot { when, saved } = &mut pin.source
                        && !*saved
                    {
                        *saved = true;
                        let (image, when) = (pin.image.clone(), *when);
                        self.keep_screenshot(image, when, false, None);
                    }
                }
                Some(Action::Open) => {
                    if let Source::File(path) = &pin.source {
                        platform::open_file(path);
                    }
                }
                None => {}
            }
            i += 1;
        }
    }
}

impl Pin {
    /// Shows the pin's window for this frame.
    fn show(&mut self, ctx: &egui::Context) -> Option<Action> {
        let title = format!("{TITLE} {}", self.id);
        let builder = ViewportBuilder::default()
            .with_title(&title)
            .with_position(self.at)
            .with_inner_size(self.size * self.zoom)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_active(false);
        self.frames += 1;
        let place = self.place.filter(|_| self.frames <= PLACE_FRAMES);
        ctx.show_viewport_immediate(ViewportId::from_hash_of(&title), builder, |ui, _| {
            if let Some((origin, scale)) = place {
                keep_at(ui.ctx(), origin, None, scale);
                ui.ctx().request_repaint();
            }
            self.ui(ui)
        })
    }

    fn ui(&mut self, ui: &mut Ui) -> Option<Action> {
        let rect = ui.max_rect();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        ui.painter().image(self.texture.id(), rect, uv, Color32::WHITE);
        ui.painter()
            .rect_stroke(rect, 0.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
        let mut action = None;
        if response.drag_started() {
            // The system moves the window from here.
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if response.double_clicked() {
            action = Some(Action::Close);
        }
        let (close, copy, save, scroll, pinch) = ui.input(|i| {
            (
                i.key_pressed(Key::Escape) || i.viewport().close_requested(),
                i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                i.modifiers.command && i.key_pressed(Key::S),
                i.smooth_scroll_delta().y,
                i.zoom_delta(),
            )
        });
        if close {
            action = Some(Action::Close);
        } else if copy {
            action = Some(Action::Copy);
        } else if save {
            action = self.save_action();
        }
        if response.hovered() {
            let factor = pinch * (scroll / 300.0).exp();
            if (factor - 1.0).abs() > 1e-3 {
                let least = (MIN_SIDE / self.size.min_elem().max(1.0)).min(1.0);
                self.zoom = (self.zoom * factor).clamp(least, MAX_ZOOM);
                self.zoomed = Some(Instant::now());
            }
        }
        if let Some(since) = self.zoomed {
            if since.elapsed() < ZOOM_SHOWN {
                zoom_tag(ui, rect, self.zoom);
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            } else {
                self.zoomed = None;
            }
        }
        response.context_menu(|ui| {
            if ui.button("Copy").clicked() {
                action = Some(Action::Copy);
                ui.close();
            }
            match &self.source {
                Source::Screenshot { saved, .. } => {
                    let label = if *saved { "Saved" } else { "Save" };
                    if ui.add_enabled(!*saved, egui::Button::new(label)).clicked() {
                        action = self.save_action();
                        ui.close();
                    }
                }
                Source::File(_) => {
                    if ui.button("Open").clicked() {
                        action = Some(Action::Open);
                        ui.close();
                    }
                }
            }
            if self.zoom != 1.0 && ui.button("Actual size").clicked() {
                self.zoom = 1.0;
                ui.close();
            }
            ui.separator();
            if ui.button("Close").clicked() {
                action = Some(Action::Close);
                ui.close();
            }
        });
        action
    }

    /// Save for a screenshot not saved yet; Open for a file.
    fn save_action(&self) -> Option<Action> {
        match &self.source {
            Source::Screenshot { saved: false, .. } => Some(Action::Save),
            Source::Screenshot { saved: true, .. } => None,
            Source::File(_) => Some(Action::Open),
        }
    }
}

/// The zoom, e.g. "150%", in the pin's top-left corner.
fn zoom_tag(ui: &Ui, rect: Rect, zoom: f32) {
    let painter = ui.painter();
    let text = format!("{:.0}%", zoom * 100.0);
    let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), Color32::WHITE);
    let tag = Align2::LEFT_TOP.anchor_size(rect.min + vec2(4.0, 4.0), galley.size() + vec2(10.0, 4.0));
    painter.rect_filled(tag, 4.0, Color32::from_black_alpha(190));
    painter.galley(tag.min + vec2(5.0, 2.0), galley, Color32::WHITE);
}
