//! A scrolling screenshot in the window: the thread that scrolls the area
//! and joins its pictures (`capture::scrolling`), a small window beside the
//! area with the height so far, Stop and Cancel, and a thin outline just
//! outside the area (`overlay::outline`); once it ends, the long picture is
//! kept like any screenshot (copied, saved, linked in the notes).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossbeam_channel::Receiver;
use eframe::egui::{self, Key, RichText, Ui, Vec2, ViewportBuilder, ViewportCommand, ViewportId, vec2};

use super::App;
use super::capture::When;
use super::overlay::{PLACE_FRAMES, beside, keep_at, outline};
use crate::capture::scrolling::{self, Area, End, Long, MAX_HEIGHT, Progress, State};
use crate::capture::{hide_from_captures, raise_overlay};

/// The control window's title, by which the system's window is found; the
/// outline's bars add a number to it.
const TITLE: &str = "Meeting Transcriber scrolling";
/// The control window's size, in points.
const SIZE: Vec2 = vec2(360.0, 124.0);
/// Between the area and the control window, in points.
const GAP: f32 = 10.0;

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
        let (window, bars) = scrolling.ready.split_at_mut(1);
        outline(ctx, &area, TITLE, bars, placing);

        let origin = beside(&area, SIZE * scale, GAP * scale);
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
        let ready = &mut window[0];
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
