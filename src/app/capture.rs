//! Screenshots in the window: taking one (the capture hotkeys, the
//! Screenshot buttons, the menu bar or tray item), after a countdown if one
//! is chosen, the overlay while choosing what to keep and drawing on it
//! (`overlay.rs`, `drawing.rs`), a scrolling screenshot of it
//! (`scrolling.rs`) or a screen recording (`recording.rs`), then copying or
//! saving it: beside the open meeting with a line in its notes, or in the
//! Screenshots folder; or pinning it to the screen (`pins.rs`). The
//! Screenshots card in Settings is `capture_settings.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, RichText, ViewportBuilder, ViewportId, pos2, vec2};

use super::overlay::{Finish, Main, Overlay, Picture, Setup};
use super::pins::Pins;
use super::recording::{Finished, Recording};
use super::scrolling::Scrolling;
use super::thumbnails::Thumbnails;
use super::{App, platform, theme};
use crate::capture::annotate::Shape;
use crate::capture::files::{self, Kind, Moment};
use crate::capture::image::Image;
use crate::capture::raster::{self, Fonts};
use crate::capture::{self, Shot, hide_from_captures, raise_overlay};
use crate::platform::Press;
use crate::settings::{CaptureSettings, ShotAction};

/// What asked for a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Trigger {
    /// A capture hotkey: which one.
    Hotkey(Press),
    /// A button in the window, which is in the way.
    Window,
    /// The 📷 above the notes: into this meeting, even a new, empty one.
    Notes,
    /// The menu bar or tray menu, which closes first.
    Menu,
}

/// How long quitting the app waits for screenshots still being written.
const SAVE_WAIT: Duration = Duration::from_secs(5);
/// The countdown's window, by whose title the system's window is found.
const COUNTDOWN_TITLE: &str = "Meeting Transcriber countdown";
/// Its size, in points.
const COUNTDOWN_SIZE: egui::Vec2 = vec2(190.0, 64.0);
/// For the countdown's window to leave the screen before the picture.
const COUNTDOWN_GONE: Duration = Duration::from_millis(200);

/// The overlay's main action (what Enter and a double-click do) for a
/// screenshot asked for by `trigger`: a capture hotkey's own, else as the
/// Settings say for a plain screenshot.
pub(super) fn main_action(trigger: Trigger, action: ShotAction) -> Main {
    match trigger {
        Trigger::Hotkey(Press::Scroll) => Main::Scroll,
        Trigger::Hotkey(Press::Record) => Main::Record,
        Trigger::Hotkey(Press::Pin) => Main::Pin,
        Trigger::Hotkey(Press::Screenshot) | Trigger::Window | Trigger::Notes | Trigger::Menu => match action {
            ShotAction::CopyAndSave | ShotAction::Copy => Main::Copy,
            ShotAction::Save => Main::Save,
            ShotAction::Pin => Main::Pin,
        },
    }
}

/// The meeting in the window, as far as where captures go is concerned.
#[derive(Debug, Clone, Copy)]
pub(super) struct OpenMeetingState {
    /// An old `.txt` transcript, which takes no notes.
    pub read_only: bool,
    pub has_file: bool,
    /// Worth a file of its own once something is added to it.
    pub worth_a_file: bool,
}

/// Whether a capture goes into the meeting in the window (beside its file,
/// with a line in its notes) rather than into its folder: asked for from
/// the notes, or with `into_meeting` set and a meeting there to go into.
pub(super) fn goes_into_meeting(open: OpenMeetingState, into_notes: bool, into_meeting: bool) -> bool {
    !open.read_only && (into_notes || (into_meeting && (open.has_file || open.worth_a_file)))
}

/// The countdown before a screenshot is taken, when one is chosen.
struct Countdown {
    until: Instant,
    /// Its window is raised and out of screen captures.
    ready: bool,
}

/// A picture being written, on its thread.
struct Saved {
    path: PathBuf,
    copied: bool,
    result: Result<()>,
    /// Said after the notice that it was saved.
    aside: Option<String>,
}

/// A line added to a meeting's notes for a file still being written, taken
/// out again if the file can't be: it would link to nothing.
struct NoteLine {
    /// The file it links.
    file: PathBuf,
    /// The meeting's file, followed when it is renamed.
    meeting: PathBuf,
    line: String,
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
    /// What Enter does on the overlay.
    main: Main,
}

pub(super) struct CaptureState {
    pub settings: CaptureSettings,
    taking: Option<Taking>,
    countdown: Option<Countdown>,
    grabbing: Option<Receiver<Result<Shot>>>,
    overlay: Option<Overlay>,
    pub scrolling: Option<Scrolling>,
    pub recording: Option<Recording>,
    /// The last recording, until its banner is dismissed.
    pub recorded: Option<Finished>,
    saved: (Sender<Saved>, Receiver<Saved>),
    /// Pictures and videos being written, so a second one does not take
    /// the same name.
    pub pending: Vec<PathBuf>,
    /// Their lines in the notes.
    lines: Vec<NoteLine>,
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
            countdown: None,
            grabbing: None,
            overlay: None,
            scrolling: None,
            recording: None,
            recorded: None,
            saved: crossbeam_channel::unbounded(),
            pending: Vec::new(),
            lines: Vec::new(),
            thumbnails: Thumbnails::default(),
            pins: Pins::default(),
            fonts: None,
            fallbacks_read: 0,
        }
    }

    /// Keeps the lines in the notes of a meeting whose file was renamed
    /// pointing at it.
    pub fn moved(&mut self, from: &Path, to: &Path) {
        for line in &mut self.lines {
            if line.meeting == from {
                line.meeting = to.to_path_buf();
            }
        }
    }
}

impl App {
    /// Takes a screenshot of the display under the mouse and shows the
    /// overlay to choose what to keep, after the countdown when one is
    /// chosen. One at a time: asked for during a countdown, a scrolling
    /// screenshot or a screen recording, it stops that one.
    pub(super) fn start_screenshot(&mut self, trigger: Trigger) {
        if self.recording_screen() {
            self.stop_recording();
            return;
        }
        if self.capture.taking.is_some() {
            if self.capture.countdown.take().is_some() {
                self.end_screenshot(&self.ctx.clone());
            } else {
                self.stop_scrolling();
            }
            return;
        }
        if let Some(why) = capture::unsupported() {
            self.report(why.into());
            self.show_window(&self.ctx.clone());
            return;
        }
        // Where the picture leaves this app's windows out by itself, the
        // window can stay.
        let in_window = matches!(trigger, Trigger::Window | Trigger::Notes);
        let hide = in_window && !capture::excludes_own_windows() && !self.dict.window_hidden;
        if hide {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        // Long enough for the window or the menu to fade out.
        let wait = match trigger {
            Trigger::Hotkey(_) => Duration::ZERO,
            Trigger::Window | Trigger::Notes if hide => Duration::from_millis(300),
            Trigger::Window | Trigger::Notes => Duration::ZERO,
            Trigger::Menu => Duration::from_millis(250),
        };
        self.capture.taking = Some(Taking {
            clock: self.recording().then(|| self.recorded_seconds()),
            show_window_after: hide,
            into_notes: trigger == Trigger::Notes,
            main: main_action(trigger, self.capture.settings.action),
        });
        let delay = Duration::from_secs(self.capture.settings.delay.into());
        if delay.is_zero() {
            self.grab_screen(wait);
        } else {
            self.capture.countdown = Some(Countdown {
                until: Instant::now() + delay.max(wait),
                ready: false,
            });
        }
    }

    /// Grabs the display under the mouse on a thread, after `wait`.
    fn grab_screen(&mut self, wait: Duration) {
        // The app in front now gets the focus back afterwards.
        capture::remember_front_app();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new().name("screenshot".into()).spawn(move || {
            std::thread::sleep(wait);
            let _ = tx.send(capture::grab());
            ctx.request_repaint();
        });
        match spawned {
            Ok(_) => self.capture.grabbing = Some(rx),
            Err(e) => {
                self.report(format!("Could not take a screenshot: {e}"));
                self.end_screenshot(&self.ctx.clone());
            }
        }
    }

    /// The screenshot hotkey, for a button's hover text: " (⌘⇧2)", or
    /// nothing when it is off.
    pub(super) fn screenshot_key(&self) -> String {
        self.capture
            .settings
            .press_hotkey(Press::Screenshot)
            .map(|key| format!(" ({})", key.label()))
            .unwrap_or_default()
    }

    /// The seconds left of the countdown, while there is one.
    pub(super) fn countdown_left(&self) -> Option<u64> {
        let left = self
            .capture
            .countdown
            .as_ref()?
            .until
            .saturating_duration_since(Instant::now());
        Some(left.as_secs_f32().ceil() as u64)
    }

    /// The overlay's choices for a screenshot whose overlay shows now.
    fn overlay_setup(&self) -> Setup {
        let settings = &self.capture.settings;
        Setup {
            main: self.capture.taking.as_ref().map_or(Main::Copy, |t| t.main),
            copy_saves: settings.action.copy_saves(),
            tool: settings.draw_tool,
            style: settings.draw_style,
        }
    }

    /// Runs every frame, also while the window is hidden: takes in the
    /// screenshot and finished saves, and keeps the overlay drawn.
    pub(super) fn capture_logic(&mut self, ctx: &egui::Context) {
        if self
            .capture
            .countdown
            .as_ref()
            .is_some_and(|c| Instant::now() >= c.until)
        {
            self.capture.countdown = None;
            self.grab_screen(COUNTDOWN_GONE);
        }
        if let Some(rx) = &self.capture.grabbing
            && let Ok(result) = rx.try_recv()
        {
            self.capture.grabbing = None;
            match result {
                Ok(shot) => self.capture.overlay = Some(Overlay::new(ctx, shot, self.overlay_setup())),
                Err(e) => {
                    self.end_screenshot(ctx);
                    self.report(format!("Could not take a screenshot: {e:#}"));
                    self.show_window(ctx);
                }
            }
        }
        self.scrolling_logic(ctx);
        self.recording_logic(ctx);
        self.pins_logic(ctx);
        let capture = &self.capture;
        if capture.overlay.is_some()
            || capture.countdown.is_some()
            || capture.scrolling.is_some()
            || capture.recording.is_some()
        {
            // The overlay, the countdown, and the scrolling and recording
            // windows, are drawn by the window's frame, which a hidden
            // window would skip.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            if capture.overlay.is_some() {
                ctx.request_repaint();
            } else if capture.scrolling.is_some() || capture.countdown.is_some() {
                ctx.request_repaint_after(Duration::from_millis(50));
            } else {
                // The recording's timer.
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
        let saved: Vec<Saved> = self.capture.saved.1.try_iter().collect();
        for s in saved {
            self.screenshot_saved(s);
        }
    }

    /// A picture written, or not.
    fn screenshot_saved(&mut self, s: Saved) {
        self.capture.pending.retain(|p| *p != s.path);
        self.file_written(&s.path, s.result.is_err());
        let name = s.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let aside = s.aside.map(|a| format!(" ({a})")).unwrap_or_default();
        match s.result {
            Ok(()) if s.copied => self.notify(format!("Copied, and saved as {name}{aside}"), Some(s.path)),
            Ok(()) => self.notify(format!("Saved {name}{aside}"), Some(s.path)),
            Err(e) => {
                self.report(format!("Could not save the screenshot {name}: {e:#}"));
                self.show_window(&self.ctx.clone());
            }
        }
    }

    /// When the app quits: stops a screen recording and waits (a little)
    /// for its file and for screenshots still being written, so none is
    /// left half done or linked from the notes without its file.
    pub(super) fn finish_captures(&mut self) {
        self.finish_recording();
        let deadline = Instant::now() + SAVE_WAIT;
        while self.capture.pending.iter().any(|p| !self.recording_to(p))
            && let Ok(s) = self.capture.saved.1.recv_deadline(deadline)
        {
            self.screenshot_saved(s);
        }
    }

    /// The overlay's window while it is up, the countdown's, the scrolling
    /// screenshot's, the screen recording's, and the pins'. Called from the
    /// window's frame.
    pub(super) fn capture_overlay(&mut self, ctx: &egui::Context) {
        self.pin_windows(ctx);
        self.countdown_window(ctx);
        self.scrolling_windows(ctx);
        self.recording_windows(ctx);
        let Some(overlay) = self.capture.overlay.as_mut() else {
            return;
        };
        let taken = overlay.taken();
        let Some(finish) = overlay.show(ctx) else {
            return;
        };
        // The colours and sizes last used, for next time.
        self.capture.settings.draw_style = overlay.draw_style();
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
            Finish::Record(area) => {
                // A window hidden for the picture stays hidden until the
                // recording ends, out of the video.
                let hidden = self.capture.taking.as_ref().is_some_and(|t| t.show_window_after);
                if hidden {
                    self.capture.taking = None;
                } else {
                    self.end_screenshot(ctx);
                }
                // Named for when it starts, not when the overlay came up.
                let when = When {
                    taken: chrono::Local::now().naive_local(),
                    clock: self.recording().then(|| self.recorded_seconds()),
                    into_notes,
                };
                return self.start_recording(area, when, hidden);
            }
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
        if copy && !self.capture.settings.action.copy_saves() {
            return self.copy_screenshot(&image);
        }
        self.keep_screenshot(image, when, copy, None);
    }

    /// Puts the picture on the clipboard only: no file, no line in the notes.
    fn copy_screenshot(&mut self, image: &Image) {
        match platform::copy_image(image.width, image.height, &image.rgba) {
            Ok(()) => self.notify("Copied to the clipboard".into(), None),
            Err(e) => {
                self.report(format!("Could not copy the screenshot: {e:#}"));
                self.show_window(&self.ctx.clone());
            }
        }
    }

    /// The countdown's small window at the top of the screen while it
    /// counts. Called from the window's frame.
    fn countdown_window(&mut self, ctx: &egui::Context) {
        let Some(left) = self.countdown_left() else {
            return;
        };
        let Some(countdown) = &mut self.capture.countdown else {
            return;
        };
        // At the top of the window's display, in the middle.
        let monitor = ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1280.0, 800.0));
        let builder = ViewportBuilder::default()
            .with_title(COUNTDOWN_TITLE)
            .with_position(pos2(((monitor.x - COUNTDOWN_SIZE.x) / 2.0).max(0.0), 48.0))
            .with_inner_size(COUNTDOWN_SIZE)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_mouse_passthrough(true)
            .with_active(false);
        let ready = &mut countdown.ready;
        ctx.show_viewport_immediate(ViewportId::from_hash_of(COUNTDOWN_TITLE), builder, |ui, _| {
            if !*ready {
                *ready = raise_overlay(COUNTDOWN_TITLE, false) && hide_from_captures(COUNTDOWN_TITLE);
            }
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(left.to_string()).size(34.0).strong());
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Screenshot…").strong());
                        ui.weak("The hotkey again cancels");
                    });
                });
            });
        });
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

    /// Where a picture or a recording goes, and the note that links it:
    /// beside the meeting in the window, if one is and captures go into
    /// meetings (its file is made now if it has none yet), else in the
    /// Screenshots or Recordings folder.
    pub(super) fn capture_place(&mut self, when: When, kind: Kind) -> Result<(PathBuf, Option<String>)> {
        let open = OpenMeetingState {
            read_only: self.open.read_only(),
            has_file: self.open.path.is_some(),
            worth_a_file: self.worth_a_file(),
        };
        if !goes_into_meeting(open, when.into_notes, self.capture.settings.into_meeting) {
            let dir = self.capture.settings.loose_dir(kind, &self.meetings_dir_path());
            let stem = Moment::Loose(when.taken).stem(kind);
            return Ok((files::unused_file(&dir, &stem, kind, &self.capture.pending), None));
        }
        if self.open.path.is_none() && !self.save_open_creating() {
            anyhow::bail!("the meeting could not be saved, so its files have no folder yet");
        }
        let meeting = self.open.path.clone().context("the meeting has no file")?;
        let moment = when.clock.map_or(Moment::Clock(when.taken), Moment::Meeting);
        let dir = files::meeting_dir(&meeting);
        let path = files::unused_file(&dir, &moment.stem(kind), kind, &self.capture.pending);
        let link = meeting
            .parent()
            .and_then(|dir| files::relative_link(dir, &path))
            .context("the file is not beside the meeting")?;
        Ok((path, Some(files::note(kind, &moment.label(kind), &link))))
    }

    /// Saves the picture (on a thread), puts it on the clipboard with
    /// `copy`, and links it in the open meeting's notes; `aside` is said
    /// once it is saved.
    pub(super) fn keep_screenshot(&mut self, image: Image, when: When, copy: bool, aside: Option<String>) {
        let (path, note) = match self.capture_place(when, Kind::Screenshot) {
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
        if let Some(note) = note {
            self.link_in_notes(&path, note);
        }
        self.capture.pending.push(path);
    }

    /// Adds `line`, which links `file` (still being written), to the open
    /// meeting's notes.
    pub(super) fn link_in_notes(&mut self, file: &Path, line: String) {
        self.open.meeting.notes = files::append_line(&self.open.meeting.notes, &line);
        self.mark_edited();
        self.save_open();
        // After saving, which renames the file to a new title.
        if let Some(meeting) = self.open.path.clone() {
            self.capture.lines.push(NoteLine {
                file: file.to_path_buf(),
                meeting,
                line,
            });
        }
    }

    /// `file` is written, or with `failed` is not: then its line goes from
    /// the notes again, the open meeting's or its file's.
    pub(super) fn file_written(&mut self, file: &Path, failed: bool) {
        let Some(i) = self.capture.lines.iter().position(|l| l.file == file) else {
            return;
        };
        let line = self.capture.lines.remove(i);
        if failed {
            self.unlink_note(&line.meeting, &line.line);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_hotkey_opens_the_overlay_with_its_own_main_action() {
        for action in [
            ShotAction::CopyAndSave,
            ShotAction::Copy,
            ShotAction::Save,
            ShotAction::Pin,
        ] {
            assert_eq!(main_action(Trigger::Hotkey(Press::Scroll), action), Main::Scroll);
            assert_eq!(main_action(Trigger::Hotkey(Press::Record), action), Main::Record);
            assert_eq!(main_action(Trigger::Hotkey(Press::Pin), action), Main::Pin);
        }
        // A plain screenshot, however it was asked for, as the Settings say.
        for trigger in [
            Trigger::Hotkey(Press::Screenshot),
            Trigger::Window,
            Trigger::Notes,
            Trigger::Menu,
        ] {
            assert_eq!(main_action(trigger, ShotAction::CopyAndSave), Main::Copy);
            assert_eq!(main_action(trigger, ShotAction::Copy), Main::Copy);
            assert_eq!(main_action(trigger, ShotAction::Save), Main::Save);
            assert_eq!(main_action(trigger, ShotAction::Pin), Main::Pin);
        }
    }

    #[test]
    fn captures_go_into_the_open_meeting_only_when_they_should() {
        let open = |read_only, has_file, worth_a_file| OpenMeetingState {
            read_only,
            has_file,
            worth_a_file,
        };
        // As before: a meeting with a file, or worth one, takes them.
        assert!(goes_into_meeting(open(false, true, false), false, true));
        assert!(goes_into_meeting(open(false, false, true), false, true));
        assert!(!goes_into_meeting(open(false, false, false), false, true));
        // The 📷 above the notes: into the meeting, even an empty new one.
        assert!(goes_into_meeting(open(false, false, false), true, true));
        // Not into meetings: always the folder, unless asked for from the notes.
        assert!(!goes_into_meeting(open(false, true, true), false, false));
        assert!(goes_into_meeting(open(false, true, true), true, false));
        // An old .txt transcript never takes them.
        assert!(!goes_into_meeting(open(true, true, true), true, true));
    }
}
