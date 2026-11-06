//! Screenshots in the window: taking one (the hotkey, the Screenshot
//! buttons, the menu bar or tray item), the overlay while choosing what to
//! keep and drawing on it (`overlay.rs`, `drawing.rs`) or a scrolling
//! screenshot of it (`scrolling.rs`), then saving it: beside the open
//! meeting with a line in its notes, or in the Screenshots folder; or
//! pinning it to the screen (`pins.rs`); and the Screenshots card in
//! Settings.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, RichText, Ui};

use super::dictation::KeysFor;
use super::overlay::{Finish, Overlay, Picture};
use super::pins::Pins;
use super::scrolling::Scrolling;
use super::thumbnails::Thumbnails;
use super::{App, platform, theme};
use crate::capture::annotate::Shape;
use crate::capture::files::{self, Moment};
use crate::capture::image::Image;
use crate::capture::raster::{self, Fonts};
use crate::capture::{self, Shot};
use crate::settings::CaptureSettings;

/// What asked for a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Trigger {
    Hotkey,
    /// A button in the window, which is in the way.
    Window,
    /// The 📷 above the notes: into this meeting, even a new, empty one.
    Notes,
    /// The menu bar or tray menu, which closes first.
    Menu,
}

/// A picture being written, on its thread.
struct Saved {
    path: PathBuf,
    copied: bool,
    result: Result<()>,
    /// Said after the notice that it was saved.
    aside: Option<String>,
}

/// When a screenshot was taken, and whether it goes into the notes.
#[derive(Debug, Clone, Copy)]
pub(super) struct When {
    taken: chrono::NaiveDateTime,
    /// The recording meeting's clock, in seconds.
    clock: Option<u64>,
    into_notes: bool,
}

/// A screenshot from the moment it was asked for until it is kept or not.
struct Taking {
    /// The recording meeting's clock then, in seconds.
    clock: Option<u64>,
    /// The window was hidden to keep it out of the picture.
    show_window_after: bool,
    /// Asked for from the notes, so it goes into them.
    into_notes: bool,
}

pub(super) struct CaptureState {
    pub settings: CaptureSettings,
    taking: Option<Taking>,
    grabbing: Option<Receiver<Result<Shot>>>,
    overlay: Option<Overlay>,
    pub scrolling: Option<Scrolling>,
    saved: (Sender<Saved>, Receiver<Saved>),
    /// Pictures being written, so a second one does not take the same name.
    pending: Vec<PathBuf>,
    pub thumbnails: Thumbnails,
    pub pins: Pins,
    /// What text drawn on screenshots is drawn in, once some is.
    fonts: Option<Fonts>,
    /// How many of the system's fallback fonts have been read into them.
    fallbacks_read: usize,
}

impl CaptureState {
    pub fn new(settings: CaptureSettings) -> Self {
        Self {
            settings,
            taking: None,
            grabbing: None,
            overlay: None,
            scrolling: None,
            saved: crossbeam_channel::unbounded(),
            pending: Vec::new(),
            thumbnails: Thumbnails::default(),
            pins: Pins::default(),
            fonts: None,
            fallbacks_read: 0,
        }
    }
}

impl App {
    /// Takes a screenshot of the display under the mouse and shows the
    /// overlay to choose what to keep. One at a time: asked for during a
    /// scrolling screenshot, it stops that one.
    pub(super) fn start_screenshot(&mut self, trigger: Trigger) {
        if self.capture.taking.is_some() {
            self.stop_scrolling();
            return;
        }
        if let Some(why) = capture::unsupported() {
            self.report(why.into());
            self.show_window(&self.ctx.clone());
            return;
        }
        // The Mac leaves this app's windows out of the picture by itself.
        let in_window = matches!(trigger, Trigger::Window | Trigger::Notes);
        let hide = in_window && !cfg!(target_os = "macos") && !self.dict.window_hidden;
        if hide {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        // Long enough for the window or the menu to fade out.
        let wait = match trigger {
            Trigger::Hotkey => Duration::ZERO,
            Trigger::Window | Trigger::Notes if hide => Duration::from_millis(300),
            Trigger::Window | Trigger::Notes => Duration::ZERO,
            Trigger::Menu => Duration::from_millis(250),
        };
        capture::remember_front_app();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new().name("screenshot".into()).spawn(move || {
            std::thread::sleep(wait);
            let _ = tx.send(capture::grab());
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            self.report(format!("Could not take a screenshot: {e}"));
            if hide {
                self.show_window(&self.ctx.clone());
            }
            return;
        }
        self.capture.taking = Some(Taking {
            clock: self.recording().then(|| self.recorded_seconds()),
            show_window_after: hide,
            into_notes: trigger == Trigger::Notes,
        });
        self.capture.grabbing = Some(rx);
    }

    /// Runs every frame, also while the window is hidden: takes in the
    /// screenshot and finished saves, and keeps the overlay drawn.
    pub(super) fn capture_logic(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.capture.grabbing
            && let Ok(result) = rx.try_recv()
        {
            self.capture.grabbing = None;
            match result {
                Ok(shot) => self.capture.overlay = Some(Overlay::new(ctx, shot)),
                Err(e) => {
                    self.end_screenshot(ctx);
                    self.report(format!("Could not take a screenshot: {e:#}"));
                    self.show_window(ctx);
                }
            }
        }
        self.scrolling_logic(ctx);
        self.pins_logic(ctx);
        if self.capture.overlay.is_some() || self.capture.scrolling.is_some() {
            // The overlay and the scrolling windows are drawn by the
            // window's frame, which a hidden window would skip.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            if self.capture.overlay.is_some() {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        let saved: Vec<Saved> = self.capture.saved.1.try_iter().collect();
        for s in saved {
            self.capture.pending.retain(|p| *p != s.path);
            let name = s.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let aside = s.aside.map(|a| format!(" ({a})")).unwrap_or_default();
            match s.result {
                Ok(()) if s.copied => self.notify(format!("Copied, and saved as {name}{aside}"), Some(s.path)),
                Ok(()) => self.notify(format!("Saved {name}{aside}"), Some(s.path)),
                Err(e) => {
                    self.report(format!("Could not save the screenshot {name}: {e:#}"));
                    self.show_window(ctx);
                }
            }
        }
    }

    /// The overlay's window while it is up, the scrolling screenshot's,
    /// and the pins'. Called from the window's frame.
    pub(super) fn capture_overlay(&mut self, ctx: &egui::Context) {
        self.pin_windows(ctx);
        self.scrolling_windows(ctx);
        let Some(overlay) = self.capture.overlay.as_mut() else {
            return;
        };
        let taken = overlay.taken();
        let Some(finish) = overlay.show(ctx) else {
            return;
        };
        self.capture.overlay = None;
        let (clock, into_notes) = self
            .capture
            .taking
            .as_ref()
            .map_or((None, false), |t| (t.clock, t.into_notes));
        let when = When {
            taken,
            clock,
            into_notes,
        };
        let (picture, copy) = match finish {
            // Still taking: the focus goes back once it is done.
            Finish::Scroll(area) => return self.start_scrolling(area, when),
            Finish::Cancel => return self.end_screenshot(ctx),
            Finish::Pin { picture, at, scale } => {
                self.end_screenshot(ctx);
                let image = self.flatten(picture);
                return self.pin_screenshot(image, at, scale, when);
            }
            Finish::Copy(picture) => (picture, true),
            Finish::Save(picture) => (picture, false),
        };
        self.end_screenshot(ctx);
        let image = self.flatten(picture);
        self.keep_screenshot(image, when, copy, None);
    }

    /// The picture with what was drawn on it, drawn into its pixels.
    fn flatten(&mut self, picture: Picture) -> Image {
        if picture.marks.is_empty() {
            return picture.image;
        }
        let texts: Vec<&str> = picture
            .marks
            .iter()
            .filter_map(|m| match &m.shape {
                Shape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let capture = &mut self.capture;
        let fonts = capture.fonts.get_or_insert_with(Fonts::builtin);
        // The system's fonts for other scripts, as the window has them,
        // read only once a text needs them.
        if !texts.iter().all(|t| fonts.covers(t)) {
            let paths = theme::fallback_font_paths();
            while capture.fallbacks_read < paths.len() && !texts.iter().all(|t| fonts.covers(t)) {
                if let Ok(bytes) = std::fs::read(&paths[capture.fallbacks_read]) {
                    fonts.add(bytes);
                }
                capture.fallbacks_read += 1;
            }
        }
        raster::render(&picture.image, &picture.marks, fonts)
    }

    /// Hands the focus back, and the window if it was hidden for the picture.
    pub(super) fn end_screenshot(&mut self, ctx: &egui::Context) {
        if let Some(taking) = self.capture.taking.take() {
            if taking.show_window_after {
                self.show_window(ctx);
            } else {
                capture::restore_front_app();
            }
        }
    }

    /// Where a picture goes, and the note that links it: beside the
    /// meeting in the window, if one is (its file is made now if it has
    /// none yet), else in the Screenshots folder.
    fn screenshot_place(&mut self, when: When) -> Result<(PathBuf, Option<String>)> {
        let in_meeting = !self.open.read_only() && (self.open.path.is_some() || self.worth_a_file() || when.into_notes);
        if !in_meeting {
            let dir = self.meetings_dir_path().join(files::LOOSE_DIR);
            let stem = Moment::Loose(when.taken).stem();
            return Ok((files::unused_png(&dir, &stem, &self.capture.pending), None));
        }
        if self.open.path.is_none() && !self.save_open_creating() {
            anyhow::bail!("the meeting could not be saved, so its pictures have no folder yet");
        }
        let meeting = self.open.path.clone().context("the meeting has no file")?;
        let moment = when.clock.map_or(Moment::Clock(when.taken), Moment::Meeting);
        let path = files::unused_png(&files::meeting_dir(&meeting), &moment.stem(), &self.capture.pending);
        let link = meeting
            .parent()
            .and_then(|dir| files::relative_link(dir, &path))
            .context("the picture is not beside the meeting")?;
        Ok((path, Some(files::image_note(&moment.label(), &link))))
    }

    /// Saves the picture (on a thread), puts it on the clipboard with
    /// `copy`, and links it in the open meeting's notes; `aside` is said
    /// once it is saved.
    pub(super) fn keep_screenshot(&mut self, image: Image, when: When, copy: bool, aside: Option<String>) {
        let (path, note) = match self.screenshot_place(when) {
            Ok(place) => place,
            Err(e) => {
                // Not lost: still on the clipboard when asked.
                if copy && let Err(e) = platform::copy_image(image.width, image.height, &image.rgba) {
                    self.report(format!("{e:#}"));
                }
                self.report(format!("Could not save the screenshot: {e:#}"));
                return;
            }
        };
        let (tx, ctx, file) = (self.capture.saved.0.clone(), self.ctx.clone(), path.clone());
        let spawned = std::thread::Builder::new()
            .name("screenshot save".into())
            .spawn(move || {
                let copied = copy && platform::copy_image(image.width, image.height, &image.rgba).is_ok();
                let result = write_png(&image, &file);
                let _ = tx.send(Saved {
                    path: file,
                    copied,
                    result,
                    aside,
                });
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            self.report(format!("Could not save the screenshot: {e}"));
            return;
        }
        self.capture.pending.push(path);
        if let Some(note) = note {
            self.open.meeting.notes = files::append_line(&self.open.meeting.notes, &note);
            self.mark_edited();
            self.save_open();
        }
    }

    /// The Screenshots card in Settings.
    pub(super) fn screenshot_card(&mut self, ui: &mut Ui) {
        theme::card_title(ui, "Screenshots");
        ui.weak(
            "Press the hotkey in any app (or use Screenshot in the sidebar or the menu), then drag over what to \
             keep, or click a window. Draw on it with the tools beside it, or Pin it to keep it on the screen. \
             Enter copies it, and it is saved either way: beside the open meeting, with a line in its notes, or \
             in the Screenshots folder of the meetings folder.",
        );
        if let Some(why) = capture::unsupported() {
            ui.colored_label(ui.visuals().warn_fg_color, why);
            return;
        }
        ui.add_space(6.0);
        let current = self.capture.settings.hotkey();
        let capturing = self.capturing_keys(KeysFor::Screenshot);
        ui.horizontal(|ui| {
            ui.label("Hotkey");
            ui.label(RichText::new(current.label()).strong());
            if capturing {
                ui.spinner();
                ui.label("Press the keys to use, then let go… (Esc cancels)");
                if ui.button("Cancel").clicked()
                    && let Some(watch) = &self.dict.hotkey
                {
                    watch.capture_next(false);
                }
                ui.ctx().request_repaint_after(Duration::from_millis(200));
            } else {
                if ui
                    .add_enabled(self.dict.hotkey.is_some(), egui::Button::new("Record keys…"))
                    .on_hover_text("Hold a combination, e.g. Ctrl+Shift+2, then let go.")
                    .on_disabled_hover_text("The keyboard can't be watched; see the message at the top.")
                    .clicked()
                {
                    self.record_keys(KeysFor::Screenshot);
                }
                if current != capture::default_hotkey() && ui.button("Reset").clicked() {
                    self.capture.settings.hotkey = capture::default_hotkey().name();
                }
            }
        });
        if current == self.dict.settings.hotkey() {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "This is also the dictation hotkey: holding it dictates too.",
            );
        }
        if self.dict.hotkey.is_none() {
            ui.weak(format!(
                "The hotkey works once the keyboard can be watched. {}",
                platform::HOTKEY_HINT
            ));
        } else if self
            .dict
            .hotkey
            .as_ref()
            .and_then(platform::HotkeyWatch::system_key)
            .is_some()
        {
            ui.weak("The desktop does not let apps watch the keyboard here, so only the Screenshot buttons work.");
        }
    }
}

/// Writes the picture as a PNG file, through a hidden temporary file so a
/// half-written one never shows.
fn write_png(image: &Image, path: &std::path::Path) -> Result<()> {
    let png = image.to_png()?;
    let dir = path.parent().context("no folder for the picture")?;
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.saving"));
    let result = std::fs::write(&tmp, png).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("could not write {}", path.display()))
}
