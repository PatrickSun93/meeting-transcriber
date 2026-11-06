//! A scrolling screenshot in the window: the thread that scrolls the area
//! and joins its pictures (`capture::scrolling`), a small window beside the
//! area with the height so far, Stop and Cancel, and a thin outline just
//! outside the area; once it ends, the long picture is kept like any
//! screenshot (copied, saved, linked in the notes).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossbeam_channel::Receiver;
use eframe::egui::{
    self, Color32, Key, Pos2, Rect, RichText, Ui, Vec2, ViewportBuilder, ViewportCommand, ViewportId, pos2, vec2,
};

use super::App;
use super::capture::When;
use super::overlay::keep_at;
use crate::capture::scrolling::{self, Area, End, Long, MAX_HEIGHT, Progress, State};
use crate::capture::{hide_from_captures, raise_overlay, select};

/// The control window's title, by which the system's window is found; the
/// outline's bars add a number to it.
const TITLE: &str = "Meeting Transcriber scrolling";
/// The control window's size, in points.
const SIZE: Vec2 = vec2(360.0, 124.0);
/// Between the area and the control window, in points.
const GAP: f32 = 10.0;
const OUTLINE: Color32 = Color32::from_rgb(40, 140, 255);
/// The outline's thickness, in points.
const THICKNESS: f32 = 2.0;
/// For how many frames the windows' places are checked and set right.
const PLACE_FRAMES: u32 = 30;

/// News from the scrolling thread.
enum Update {
    Progress(Progress),
    Done(Result<Long>),
}

/// A scrolling screenshot from Scroll on the overlay until it is kept.
pub(super) struct Scrolling {
    area: Area,
    when: When,
    stop: Arc<AtomicBool>,
    updates: Receiver<Update>,
    progress: Progress,
    /// Stopped with Cancel: the picture is not kept.
    cancelled: bool,
    frames: u32,
    /// Whether each window is raised and out of the pictures yet: the
    /// control window, then the outline's four bars.
    ready: [bool; 5],
}

impl Scrolling {
    /// Asks the thread to finish; without `keep`, what it took is dropped.
    fn stop(&mut self, keep: bool) {
        self.cancelled |= !keep;
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl App {
    /// Starts a scrolling screenshot of `area`, on a thread.
    pub(super) fn start_scrolling(&mut self, area: Area, when: When) {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = crossbeam_channel::unbounded();
        let (ctx, flag) = (self.ctx.clone(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("scrolling screenshot".into())
            .spawn(move || {
                let result = scrolling::run(area, &flag, |p| {
                    let _ = tx.send(Update::Progress(p));
                    ctx.request_repaint();
                });
                let _ = tx.send(Update::Done(result));
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            self.report(format!("Could not take a scrolling screenshot: {e}"));
            self.end_screenshot(&self.ctx.clone());
            return;
        }
        self.capture.scrolling = Some(Scrolling {
            area,
            when,
            stop,
            updates: rx,
            progress: Progress {
                height: area.rect.height,
                state: State::Scrolling,
            },
            cancelled: false,
            frames: 0,
            ready: [false; 5],
        });
    }

    /// Stops the scrolling screenshot, if one is going, keeping what it took.
    pub(super) fn stop_scrolling(&mut self) {
        if let Some(scrolling) = &mut self.capture.scrolling {
            scrolling.stop(true);
        }
    }

    /// Takes in the thread's news, and the picture once it is done.
    pub(super) fn scrolling_logic(&mut self, ctx: &egui::Context) {
        let Some(scrolling) = &mut self.capture.scrolling else {
            return;
        };
        let mut done = None;
        for update in scrolling.updates.try_iter() {
            match update {
                Update::Progress(p) => scrolling.progress = p,
                Update::Done(result) => done = Some(result),
            }
        }
        let Some(result) = done else {
            return;
        };
        let (when, cancelled) = (scrolling.when, scrolling.cancelled);
        self.capture.scrolling = None;
        self.end_screenshot(ctx);
        match result {
            Ok(_) if cancelled => {}
            Ok(long) => {
                let aside = (long.end == End::Limit).then(|| format!("it stopped at the {MAX_HEIGHT}-pixel limit"));
                self.keep_screenshot(long.image, when, true, aside);
            }
            Err(e) => {
                self.report(format!("Could not take a scrolling screenshot: {e:#}"));
                self.show_window(ctx);
            }
        }
    }

    /// The control window and the outline, while scrolling. Called from
    /// the window's frame.
    pub(super) fn scrolling_windows(&mut self, ctx: &egui::Context) {
        let Some(scrolling) = &mut self.capture.scrolling else {
            return;
        };
        scrolling.frames += 1;
        let placing = scrolling.frames <= PLACE_FRAMES;
        let area = scrolling.area;
        let scale = area.display.scale.max(0.25);
        let thickness = (THICKNESS * scale).round().max(1.0) as i32;
        for (i, [x, y, width, height]) in area.outline(thickness).into_iter().enumerate() {
            let title = format!("{TITLE} {}", i + 1);
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
            let ready = &mut scrolling.ready[i + 1];
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

        let origin = control_origin(&area, SIZE * scale, GAP * scale);
        let builder = ViewportBuilder::default()
            .with_title(TITLE)
            .with_position((origin.to_vec2() / scale).to_pos2())
            .with_inner_size(SIZE)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_active(true);
        let progress = scrolling.progress;
        let ready = &mut scrolling.ready[0];
        let chosen = ctx.show_viewport_immediate(ViewportId::from_hash_of(TITLE), builder, |ui, _| {
            if !*ready {
                *ready = raise_overlay(TITLE, true) && hide_from_captures(TITLE);
                if *ready {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
                }
            }
            if placing {
                keep_at(ui.ctx(), origin, None);
            }
            control(ui, progress)
        });
        if let Some(keep) = chosen {
            scrolling.stop(keep);
        }
    }
}

/// Where the control window goes on the desktop, in its pixels: under the
/// area, else above it, else inside it at the bottom right; on its display.
fn control_origin(area: &Area, size: Vec2, gap: f32) -> Pos2 {
    let r = area.rect;
    let rect = Rect::from_min_size(pos2(r.x as f32, r.y as f32), vec2(r.width as f32, r.height as f32));
    let [width, height] = area.display_size;
    let display = Rect::from_min_size(Pos2::ZERO, vec2(width as f32, height as f32));
    let [x, y] = area.display.origin_px;
    select::toolbar_origin(rect, size, display, gap) + vec2(x as f32, y as f32)
}

/// The control window's content; Some when it is to stop: keeping the
/// picture (Stop, Esc, Enter) or not (Cancel).
fn control(ui: &mut Ui, progress: Progress) -> Option<bool> {
    let mut chosen = ui
        .input(|i| i.key_pressed(Key::Escape) || i.key_pressed(Key::Enter) || i.viewport().close_requested())
        .then_some(true);
    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(format!("Scrolling…  {} px", progress.height)).strong());
        });
        ui.label(RichText::new(say(progress.state)).small().weak());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .button("Stop")
                .on_hover_text("Keep it: copied and saved (Esc, Enter, or the screenshot hotkey)")
                .clicked()
            {
                chosen = Some(true);
            }
            if ui.button("Cancel").on_hover_text("Throw it away").clicked() {
                chosen = Some(false);
            }
        });
    });
    chosen
}

/// What the scrolling is doing, in words.
fn say(state: State) -> &'static str {
    match state {
        State::Scrolling => "Scrolls down by itself and stops at the end of the page. You can scroll too.",
        State::Paused => "Paused: move the pointer back over the area, or scroll it by hand.",
        State::ByHand(why) => why,
        State::Lost => "Lost its place: scroll back up a little, then on more slowly.",
    }
}
