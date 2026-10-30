//! The open meeting and the meetings folder: opening, autosaving, renaming,
//! deleting, converting old transcripts, exporting, and noticing edits made
//! to the files elsewhere.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context as _;
use crossbeam_channel::Receiver;
use eframe::egui;

use super::path_prompt::PromptFor;
use super::{App, Page, platform};
use crate::meeting::{self, Meeting};
use crate::meetings::{self, Kind, Library};
use crate::transcript::Line;

/// Quiet time after the last change before it is written.
const AUTOSAVE_DELAY: Duration = Duration::from_secs(1);
/// Longest a change waits, even while the user keeps typing.
const AUTOSAVE_MAX_DELAY: Duration = Duration::from_secs(5);
/// How often the meetings folder is checked for changes made elsewhere.
const RESCAN_EVERY: Duration = Duration::from_secs(3);
/// How long a notice such as "Exported …" stays in the footer.
const NOTICE_FOR: Duration = Duration::from_secs(6);
pub(super) const DEFAULT_TITLE: &str = "Meeting";

/// The meeting in the window. Its transcript lines live in `App::transcript`.
pub(super) struct OpenMeeting {
    pub meeting: Meeting,
    /// None until the first save.
    pub path: Option<PathBuf>,
    pub kind: Kind,
    /// Base id for the meeting's widgets (title, notes, scroll positions).
    /// Fixed while the meeting is open, so the first save or a rename does
    /// not take focus from the field being typed in, and unique per meeting,
    /// so undo in one meeting's notes never brings back another's text.
    pub view_id: egui::Id,
    /// The title the file is named after; changing the title renames it.
    saved_title: String,
    /// The tags field as typed.
    pub tags_text: String,
    /// The file's modification time when we last read or wrote it.
    disk_modified: Option<SystemTime>,
    /// Title, tags or notes changed here and not saved yet.
    edited: bool,
    /// Transcript lines not saved yet.
    new_lines: bool,
    /// When the last unsaved change was made, for the autosave delay.
    changed_at: Option<Instant>,
    /// When the oldest unsaved change was made.
    unsaved_since: Option<Instant>,
}

impl OpenMeeting {
    pub fn unsaved() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let meeting = Meeting::new(DEFAULT_TITLE, chrono::Local::now().naive_local());
        let mut open = Self::from_meeting(meeting, None, Kind::Markdown, None);
        open.view_id = egui::Id::unique(("new meeting", NEXT.fetch_add(1, Ordering::Relaxed)));
        open
    }

    fn from_meeting(meeting: Meeting, path: Option<PathBuf>, kind: Kind, disk_modified: Option<SystemTime>) -> Self {
        // By path, so a reopened meeting gets its scroll position back.
        let view_id = egui::Id::unique(("meeting", &path));
        Self {
            view_id,
            saved_title: meeting.title.clone(),
            tags_text: meeting.tags.join(", "),
            meeting,
            path,
            kind,
            disk_modified,
            edited: false,
            new_lines: false,
            changed_at: None,
            unsaved_since: None,
        }
    }

    /// Old plain-text transcripts are shown but never written.
    pub fn read_only(&self) -> bool {
        self.kind == Kind::Text
    }

    pub fn unsaved_changes(&self) -> bool {
        self.edited || self.new_lines
    }
}

/// The sidebar's view of the meetings folder.
#[derive(Default)]
pub(super) struct MeetingList {
    pub library: Library,
    pub search: String,
    /// A meeting being renamed in the sidebar, with the title typed so far.
    pub renaming: Option<(PathBuf, String)>,
    /// A meeting whose Delete was chosen once, awaiting confirmation.
    pub confirm_delete: Option<PathBuf>,
    dir: PathBuf,
    scanned: Option<Instant>,
    focused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Markdown,
    Text,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Text => "txt",
        }
    }
}

/// The folder dialog for an export, open on a background thread.
pub(super) struct ExportPick {
    format: Format,
    result: Receiver<platform::Picked>,
}

/// A short message in the footer, optionally with a file to show.
pub(super) struct Notice {
    pub text: String,
    pub file: Option<PathBuf>,
    shown: Instant,
}

impl App {
    /// The open meeting as it would be saved now.
    pub(super) fn current_meeting(&self) -> Meeting {
        let mut m = self.open.meeting.clone();
        m.title = meeting::one_line(&m.title);
        if m.title.is_empty() {
            m.title = DEFAULT_TITLE.into();
        }
        m.lines = self.transcript.lines().to_vec();
        m.languages = meeting::merge_languages(&m.languages, &m.lines);
        m.duration = m.duration.max(self.recorded_seconds());
        m
    }

    /// How far the meeting's clock has run: its last line, or the live
    /// recording.
    pub(super) fn recorded_seconds(&self) -> u64 {
        let last = self.transcript.lines().last().map_or(0.0, |l| l.end);
        let live = self
            .session
            .as_ref()
            .map_or(0.0, |s| self.session_offset + s.started.elapsed().as_secs_f32());
        last.max(live).ceil() as u64
    }

    /// Title, tags or notes changed.
    pub(super) fn mark_edited(&mut self) {
        self.open.edited = true;
        self.open.changed_at = Some(Instant::now());
        self.open.unsaved_since.get_or_insert_with(Instant::now);
        // The frame that saves; nothing else may come along to draw it.
        self.ctx.request_repaint_after(AUTOSAVE_DELAY);
    }

    /// Transcript lines were added. Saved a moment after the first one, so
    /// a steady stream of lines still reaches the file every second.
    pub(super) fn mark_new_lines(&mut self) {
        self.open.new_lines = true;
        self.open.changed_at.get_or_insert_with(Instant::now);
        self.open.unsaved_since.get_or_insert_with(Instant::now);
        self.ctx.request_repaint_after(AUTOSAVE_DELAY);
    }

    /// Saves once changes have been quiet for a moment.
    pub(super) fn autosave(&mut self) {
        let (Some(at), Some(since)) = (self.open.changed_at, self.open.unsaved_since) else {
            return;
        };
        let wait = AUTOSAVE_DELAY
            .saturating_sub(at.elapsed())
            .min(AUTOSAVE_MAX_DELAY.saturating_sub(since.elapsed()));
        if wait.is_zero() {
            self.save_open();
        } else {
            self.ctx.request_repaint_after(wait);
        }
    }

    /// A new meeting gets a file once it has something in it.
    fn worth_a_file(&self) -> bool {
        let m = &self.open.meeting;
        self.recording()
            || !self.transcript.is_empty()
            || !m.notes.trim().is_empty()
            || !m.tags.is_empty()
            || m.title != DEFAULT_TITLE
    }

    /// Writes the open meeting now if it has unsaved changes. False if it
    /// could not be written, so callers keep it open rather than drop the
    /// user's notes.
    pub(super) fn save_open(&mut self) -> bool {
        !self.open.unsaved_changes() || self.write_open(false)
    }

    /// Writes the open meeting, creating its file even when it is empty
    /// (recording needs a file for lines that finish late).
    pub(super) fn save_open_creating(&mut self) -> bool {
        self.write_open(true)
    }

    fn write_open(&mut self, create: bool) -> bool {
        self.open.changed_at = None;
        self.open.unsaved_since = None;
        if self.open.read_only() || (self.open.path.is_none() && !create && !self.worth_a_file()) {
            self.open.edited = false;
            self.open.new_lines = false;
            return true;
        }
        match self.try_write_open() {
            Ok(()) => {
                self.open.edited = false;
                self.open.new_lines = false;
                true
            }
            Err(e) => {
                self.report(format!(
                    "Could not save the meeting, so it stays open (Copy all keeps a copy of your notes): {e:#}"
                ));
                // Try again shortly: the folder may come back (a disk
                // plugged in again, say).
                self.open.changed_at = Some(Instant::now());
                self.open.unsaved_since = Some(Instant::now());
                false
            }
        }
    }

    fn try_write_open(&mut self) -> anyhow::Result<()> {
        if self.open.path.is_none() {
            // A meeting is dated by when its file was started, not when the
            // window was opened.
            self.open.meeting.date = chrono::Local::now().naive_local();
        }
        let m = self.current_meeting();
        let stem = meetings::file_stem(m.date, &m.title);
        let path = match self.open.path.clone() {
            None => meetings::unused_path(&self.meetings_dir_path(), &stem, "md", None),
            Some(old) if m.title != self.open.saved_title => self.rename_file(&old, &stem)?,
            Some(old) => old,
        };
        write_atomic(&path, &m.to_markdown()).with_context(|| format!("writing {}", path.display()))?;
        self.open.meeting.duration = m.duration;
        self.open.meeting.languages = m.languages;
        self.open.saved_title = m.title;
        self.open.disk_modified = modified_time(&path);
        self.open.path = Some(path);
        self.list.scanned = None;
        Ok(())
    }

    /// Renames a meeting file to `stem`, keeping its folder.
    fn rename_file(&mut self, from: &Path, stem: &str) -> anyhow::Result<PathBuf> {
        let dir = from.parent().unwrap_or(Path::new("."));
        let to = meetings::unused_path(dir, stem, "md", Some(from));
        // A file removed elsewhere is simply written again under the new name.
        if to != from && from.exists() {
            std::fs::rename(from, &to).with_context(|| format!("renaming {}", from.display()))?;
        }
        self.moved(from, &to);
        Ok(to)
    }

    /// Keeps references to a renamed file pointing at it.
    fn moved(&mut self, from: &Path, to: &Path) {
        for (_, path) in &mut self.session_files {
            if path == from {
                *path = to.to_path_buf();
            }
        }
        if self.open.path.as_deref() == Some(from) {
            self.open.path = Some(to.to_path_buf());
        }
    }

    /// Shows an error once, however often it happens.
    pub(super) fn report(&mut self, message: String) {
        if !self.errors.contains(&message) {
            self.errors.push(message);
        }
    }

    pub(super) fn notify(&mut self, text: String, file: Option<PathBuf>) {
        self.notice = Some(Notice {
            text,
            file,
            shown: Instant::now(),
        });
        self.ctx.request_repaint_after(NOTICE_FOR);
    }

    /// The footer notice, while it is fresh.
    pub(super) fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref().filter(|n| n.shown.elapsed() < NOTICE_FOR)
    }

    /// Replaces the window's meeting, keeping the reader's search.
    fn show(&mut self, open: OpenMeeting, lines: Vec<Line>) {
        self.open = open;
        self.transcript.set_lines(lines);
        self.reader.opened();
        self.page = Page::Meeting;
    }

    /// Starts an empty meeting. Its file is made once it has content.
    pub(super) fn new_meeting(&mut self) {
        if self.recording() || !self.save_open() {
            return;
        }
        self.show(OpenMeeting::unsaved(), Vec::new());
    }

    /// Opens a meeting from the list (not while recording another).
    pub(super) fn open_meeting(&mut self, path: &Path, kind: Kind) {
        if self.open.path.as_deref() == Some(path) {
            self.page = Page::Meeting;
            return;
        }
        if self.recording() || !self.save_open() {
            return;
        }
        match load(path, kind) {
            Ok((open, lines)) => self.show(open, lines),
            Err(e) => self.report(format!("{e:#}")),
        }
    }

    /// Renames a meeting from the sidebar, open or not.
    pub(super) fn rename_meeting(&mut self, path: &Path, title: &str) {
        let title = meeting::one_line(title);
        if title.is_empty() {
            return;
        }
        if self.open.path.as_deref() == Some(path) {
            self.open.meeting.title = title;
            self.mark_edited();
            self.save_open();
            return;
        }
        let result = (|| -> anyhow::Result<()> {
            let (mut open, lines) = load(path, Kind::Markdown)?;
            open.meeting.title = title;
            open.meeting.lines = lines;
            let to = self.rename_file(path, &meetings::file_stem(open.meeting.date, &open.meeting.title))?;
            write_atomic(&to, &open.meeting.to_markdown()).with_context(|| format!("writing {}", to.display()))?;
            Ok(())
        })();
        if let Err(e) = result {
            self.report(format!("Could not rename the meeting: {e:#}"));
        }
        self.list.scanned = None;
    }

    /// Removes a meeting's file (after the sidebar's confirmation).
    pub(super) fn delete_meeting(&mut self, path: &Path) {
        self.list.confirm_delete = None;
        let is_open = self.open.path.as_deref() == Some(path);
        if is_open && self.recording() {
            return;
        }
        if let Err(e) = std::fs::remove_file(path) {
            self.report(format!("Could not delete {}: {e}", path.display()));
            return;
        }
        if is_open {
            self.show(OpenMeeting::unsaved(), Vec::new());
        }
        self.list.scanned = None;
    }

    /// Makes a Markdown meeting from the open old transcript, which is kept.
    pub(super) fn convert_to_markdown(&mut self) {
        let Some(txt) = self.open.path.clone().filter(|_| self.open.read_only()) else {
            return;
        };
        let mut m = self.current_meeting();
        m.title = DEFAULT_TITLE.into();
        let dir = txt.parent().unwrap_or(Path::new("."));
        let to = meetings::unused_path(dir, &meetings::file_stem(m.date, &m.title), "md", None);
        match write_atomic(&to, &m.to_markdown()) {
            Ok(()) => {
                self.list.scanned = None;
                self.open_meeting(&to, Kind::Markdown);
                let name = file_name(&to);
                self.notify(format!("Converted to {name}. The .txt file is kept."), Some(to));
            }
            Err(e) => self.report(format!("Could not write {}: {e}", to.display())),
        }
    }

    /// The open meeting in an export format.
    pub(super) fn export_text(&self, format: Format) -> String {
        match format {
            Format::Markdown => self.current_meeting().to_markdown(),
            Format::Text => self.transcript.to_text(),
        }
    }

    /// Writes the open meeting as `format` into `dir`, never overwriting.
    pub(super) fn export_to(&mut self, format: Format, dir: &Path) {
        let m = &self.open.meeting;
        let stem = self.open.path.as_ref().and_then(|p| p.file_stem()).map_or_else(
            || meetings::file_stem(m.date, &m.title),
            |s| s.to_string_lossy().into_owned(),
        );
        // The meeting itself is already that file.
        if let Some(path) = self.open.path.clone()
            && path.parent() == Some(dir)
            && path.extension().is_some_and(|e| e == format.extension())
        {
            self.save_open();
            self.notify(format!("{} is already in that folder.", file_name(&path)), Some(path));
            return;
        }
        let to = meetings::unused_path(dir, &stem, format.extension(), None);
        match write_atomic(&to, &self.export_text(format)) {
            Ok(()) => {
                self.notify(format!("Exported {}", file_name(&to)), Some(to));
                self.list.scanned = None;
            }
            Err(e) => self.report(format!("Could not export to {}: {e}", to.display())),
        }
    }

    /// The meeting's own folder, for "Save next to the meeting".
    pub(super) fn open_meeting_dir(&self) -> PathBuf {
        self.open
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map_or_else(|| self.meetings_dir_path(), Path::to_path_buf)
    }

    fn export_prompt(format: Format) -> String {
        format!("Export the meeting as .{}", format.extension())
    }

    pub(super) fn pick_export_folder(&mut self, format: Format) {
        let ctx = self.ctx.clone();
        let result = platform::pick_folder(&Self::export_prompt(format), &self.open_meeting_dir(), move || {
            ctx.request_repaint()
        });
        self.export_pick = Some(ExportPick { format, result });
    }

    pub(super) fn exporting(&self) -> bool {
        self.export_pick.is_some()
    }

    pub(super) fn poll_export_pick(&mut self) {
        if let Some(pick) = &self.export_pick
            && let Ok(chosen) = pick.result.try_recv()
        {
            let format = pick.format;
            self.export_pick = None;
            let start = self.open_meeting_dir();
            self.picked(PromptFor::Export(format), &Self::export_prompt(format), &start, chosen);
        }
    }

    /// Re-lists the meetings folder every few seconds, when the window
    /// regains focus, or right after a change, and reloads the open meeting
    /// if its file was edited elsewhere.
    pub(super) fn refresh_library(&mut self) {
        let dir = self.meetings_dir_path();
        let focused = self.ctx.input(|i| i.focused);
        let regained = focused && !self.list.focused;
        self.list.focused = focused;
        let due = self.list.scanned.is_none_or(|t| t.elapsed() >= RESCAN_EVERY);
        if due || regained || self.list.dir != dir {
            self.list.library.scan(&dir);
            self.list.dir = dir;
            self.list.scanned = Some(Instant::now());
            self.reload_if_changed_on_disk();
        }
        self.ctx.request_repaint_after(RESCAN_EVERY);
    }

    /// Takes in edits made to the open file elsewhere (Obsidian, say),
    /// unless there are unsaved edits here, which are written over them.
    fn reload_if_changed_on_disk(&mut self) {
        let Some(path) = self.open.path.clone() else { return };
        let modified = modified_time(&path);
        if modified.is_none() || modified == self.open.disk_modified || self.open.edited {
            return;
        }
        let Ok((fresh, lines)) = load(&path, self.open.kind) else {
            return;
        };
        // Lines recorded here and not saved yet are newer than the file's.
        let keep_lines = self.open.new_lines || self.recording();
        let open = &self.open;
        let (view_id, new_lines, changed_at, unsaved_since) =
            (open.view_id, open.new_lines, open.changed_at, open.unsaved_since);
        self.open = OpenMeeting {
            view_id,
            new_lines,
            changed_at,
            unsaved_since,
            ..fresh
        };
        if !keep_lines {
            self.transcript.set_lines(lines);
        }
    }

    /// The file a session's lines belong in, found by utterance id (each
    /// session starts a new id range).
    fn session_file(&self, utterance: u64) -> Option<&Path> {
        self.session_files
            .iter()
            .rev()
            .find(|(first, _)| *first <= utterance)
            .map(|(_, p)| p.as_path())
    }

    /// Whether a transcription result belongs to the meeting in the window.
    /// Results of an earlier session can arrive after the user has opened
    /// another meeting.
    pub(super) fn for_open_meeting(&self, utterance: u64) -> bool {
        self.session_file(utterance)
            .is_none_or(|p| self.open.path.as_deref() == Some(p))
    }

    /// Keeps a line for the meeting it was recorded in.
    pub(super) fn add_late_line(&mut self, utterance: u64, line: Line) {
        if let Some(path) = self.session_file(utterance) {
            self.late_lines.push((path.to_path_buf(), line));
        }
    }

    /// Writes lines that finished after their meeting was closed into its file.
    pub(super) fn flush_late_lines(&mut self) {
        let mut by_file: BTreeMap<PathBuf, Vec<Line>> = BTreeMap::new();
        for (path, line) in self.late_lines.drain(..) {
            by_file.entry(path).or_default().push(line);
        }
        for (path, lines) in by_file {
            // Deleted meanwhile: the user did not want it.
            if !path.exists() {
                continue;
            }
            if let Err(e) = append_lines(&path, lines) {
                self.report(format!("Could not add the last lines to {}: {e:#}", path.display()));
            }
            self.list.scanned = None;
        }
    }
}

/// Reads a meeting file: Markdown, or an old transcript as a read-only meeting.
fn load(path: &Path, kind: Kind) -> anyhow::Result<(OpenMeeting, Vec<Line>)> {
    let text = std::fs::read_to_string(path).with_context(|| format!("Could not open {}", path.display()))?;
    let modified = modified_time(path);
    let name = file_name(path);
    let date = meetings::parse_stamp(&name)
        .or_else(|| modified.map(meetings::local_time))
        .unwrap_or_else(|| chrono::Local::now().naive_local());
    let title = meetings::title_from_name(&name);
    let mut m = match kind {
        Kind::Markdown => Meeting::parse(&text, &title, date),
        Kind::Text => {
            let mut m = Meeting::new(&title, date);
            m.lines = meeting::parse_legacy_text(&text);
            m.duration = m.lines.last().map_or(0, |l| l.start.ceil() as u64);
            m
        }
    };
    let lines = std::mem::take(&mut m.lines);
    Ok((
        OpenMeeting::from_meeting(m, Some(path.to_path_buf()), kind, modified),
        lines,
    ))
}

/// Adds lines to a saved meeting, in time order.
fn append_lines(path: &Path, lines: Vec<Line>) -> anyhow::Result<()> {
    let (open, existing) = load(path, Kind::Markdown)?;
    let mut m = open.meeting;
    m.lines = existing;
    m.lines.extend(lines);
    m.lines.sort_by(|a, b| a.start.total_cmp(&b.start));
    m.languages = meeting::merge_languages(&m.languages, &m.lines);
    if let Some(last) = m.lines.last() {
        m.duration = m.duration.max(last.end.ceil() as u64);
    }
    write_atomic(path, &m.to_markdown()).with_context(|| format!("writing {}", path.display()))
}

/// Writes through a hidden temporary file and renames it into place, so a
/// crash mid-write never leaves half a meeting behind.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.saving", file_name(path)));
    let result = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Speaker;

    fn line(speaker: Speaker, start: f32, text: &str) -> Line {
        Line {
            speaker,
            start,
            end: start + 1.0,
            text: text.into(),
            language: Some("en".into()),
        }
    }

    #[test]
    fn late_lines_are_merged_into_the_file() {
        let dir = std::env::temp_dir().join(format!("meeting-transcriber-test-{}-late", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("2026-10-29 14-05 Sync.md");
        let mut m = Meeting::new("Sync", meeting::parse_date("2026-10-29T14:05").unwrap());
        m.notes = "keep me".into();
        m.lines = vec![line(Speaker::Me, 1.0, "first"), line(Speaker::Me, 9.0, "third")];
        write_atomic(&path, &m.to_markdown()).unwrap();
        assert!(!dir.join(".2026-10-29 14-05 Sync.md.saving").exists());

        append_lines(
            &path,
            vec![line(Speaker::Others, 5.0, "second"), line(Speaker::Me, 20.0, "last")],
        )
        .unwrap();
        let (open, lines) = load(&path, Kind::Markdown).unwrap();
        let texts: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["first", "second", "third", "last"]);
        assert_eq!(open.meeting.notes, "keep me");
        assert_eq!(open.meeting.duration, 21);
        assert_eq!(open.meeting.languages, ["en"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_transcripts_load_read_only() {
        let dir = std::env::temp_dir().join(format!("meeting-transcriber-test-{}-legacy", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Meeting 2026-10-29 14-05.txt");
        std::fs::write(&path, "[00:03] Me: Hi.\n[00:07] Others: Hello.\n").unwrap();
        let (open, lines) = load(&path, Kind::Text).unwrap();
        assert!(open.read_only());
        assert_eq!(open.meeting.date, meeting::parse_date("2026-10-29T14:05").unwrap());
        assert_eq!(lines.len(), 2);
        assert_eq!(open.meeting.duration, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
