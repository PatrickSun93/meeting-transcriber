//! The meetings folder: file names, listing, search and date groups.
//!
//! Each meeting is `YYYY-MM-DD HH-MM <Title>.md` (see `meeting`). Plain-text
//! transcripts from earlier versions (`Meeting <date> <time>.txt`) are listed
//! too, and left untouched.

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{Datelike as _, NaiveDate, NaiveDateTime};

use crate::meeting;

/// File names carry this much of the title; the rest is in the file.
const MAX_TITLE_CHARS: usize = 80;
const STAMP_FORMAT: &str = "%Y-%m-%d %H-%M";
const LEGACY_PREFIX: &str = "Meeting ";

/// `~/Documents/Meeting Transcripts`.
pub fn default_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
        .join("Meeting Transcripts")
}

/// A title made safe for a file name: no path separators or characters
/// Finder and other systems reject, single spaces, no leading dot, and not
/// too long. Empty when nothing usable is left.
pub fn sanitize_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let single: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = single.trim_start_matches('.').chars().take(MAX_TITLE_CHARS).collect();
    short.trim().trim_end_matches('.').trim().to_string()
}

/// `2026-10-29 14-05 Weekly sync`, without extension.
pub fn file_stem(date: NaiveDateTime, title: &str) -> String {
    let stamp = date.format(STAMP_FORMAT).to_string();
    match sanitize_title(title) {
        t if t.is_empty() => stamp,
        t => format!("{stamp} {t}"),
    }
}

/// `<dir>/<stem>.<ext>`, or `<stem> (2).<ext>` and so on when taken, so a
/// file is never overwritten. `current` (the file being renamed) counts as
/// free, which keeps a meeting's name stable and allows case-only renames.
pub fn unused_path(dir: &Path, stem: &str, ext: &str, current: Option<&Path>) -> PathBuf {
    let is_current = |p: &Path| {
        current.is_some_and(|c| {
            c.parent() == p.parent()
                && c.file_name().map(|n| n.to_string_lossy().to_lowercase())
                    == p.file_name().map(|n| n.to_string_lossy().to_lowercase())
        })
    };
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() && !is_current(&path) {
        path = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    path
}

/// The start time in a file name: `2026-10-29 14-05 …` or the old
/// `Meeting 2026-10-29 14-05….txt`.
pub fn parse_stamp(file_name: &str) -> Option<NaiveDateTime> {
    let name = file_name.strip_prefix(LEGACY_PREFIX).unwrap_or(file_name);
    let stamp = name.get(..16)?;
    NaiveDateTime::parse_from_str(stamp, STAMP_FORMAT).ok()
}

/// An old plain-text transcript, as written before meetings were Markdown.
pub fn is_legacy_name(file_name: &str) -> bool {
    file_name.starts_with(LEGACY_PREFIX) && file_name.ends_with(".txt") && parse_stamp(file_name).is_some()
}

/// The title a file name suggests when the file itself has none.
pub fn title_from_name(file_name: &str) -> String {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    let title = if file_name.starts_with(LEGACY_PREFIX) {
        stem
    } else if parse_stamp(stem).is_some() {
        stem[16..].trim()
    } else {
        stem
    };
    if title.is_empty() {
        "Meeting".into()
    } else {
        title.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A meeting file this app writes and edits.
    Markdown,
    /// An old plain-text transcript, shown read-only.
    Text,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
    pub title: String,
    pub date: NaiveDateTime,
    pub tags: Vec<String>,
}

impl Entry {
    /// Search on title and tags: every word must match one of them. A word
    /// starting with `#` only matches tags.
    pub fn matches(&self, query: &str) -> bool {
        let title = self.title.to_lowercase();
        let tags: Vec<String> = self.tags.iter().map(|t| t.to_lowercase()).collect();
        query.split_whitespace().all(|word| {
            let word = word.to_lowercase();
            match word.strip_prefix('#') {
                Some(tag) => tags.iter().any(|t| t.contains(tag)),
                None => title.contains(&word) || tags.iter().any(|t| t.contains(&word)),
            }
        })
    }
}

/// The meetings in a folder, re-read only for files that changed.
#[derive(Default)]
pub struct Library {
    pub entries: Vec<Entry>,
    cache: HashMap<PathBuf, (SystemTime, u64, Entry)>,
}

impl Library {
    /// Lists `dir`, newest first. Returns true if anything changed.
    pub fn scan(&mut self, dir: &Path) -> bool {
        let mut cache = HashMap::new();
        let mut entries = Vec::new();
        for item in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = item.path();
            let name = item.file_name().to_string_lossy().into_owned();
            let kind = if name.starts_with('.') {
                continue;
            } else if name.ends_with(".md") {
                Kind::Markdown
            } else if is_legacy_name(&name) {
                Kind::Text
            } else {
                continue;
            };
            let Some(meta) = std::fs::metadata(&path).ok().filter(|m| m.is_file()) else {
                continue;
            };
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let entry = match self.cache.remove(&path) {
                Some((m, len, entry)) if m == modified && len == meta.len() => entry,
                _ => read_entry(&path, &name, kind, modified),
            };
            entries.push(entry.clone());
            cache.insert(path, (modified, meta.len(), entry));
        }
        sort_newest_first(&mut entries);
        self.cache = cache;
        let changed = entries != self.entries;
        self.entries = entries;
        changed
    }
}

pub fn sort_newest_first(entries: &mut [Entry]) {
    entries.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.title.cmp(&b.title)));
}

fn read_entry(path: &Path, name: &str, kind: Kind, modified: SystemTime) -> Entry {
    let header = match kind {
        Kind::Markdown => std::fs::File::open(path)
            .map(|f| meeting::read_header(BufReader::new(f)))
            .unwrap_or_default(),
        Kind::Text => meeting::Header::default(),
    };
    Entry {
        path: path.to_path_buf(),
        kind,
        title: header.title.unwrap_or_else(|| title_from_name(name)),
        date: header
            .date
            .or_else(|| parse_stamp(name))
            .unwrap_or_else(|| local_time(modified)),
        tags: header.tags,
    }
}

/// A file time as local wall-clock time.
pub fn local_time(t: SystemTime) -> NaiveDateTime {
    chrono::DateTime::<chrono::Local>::from(t).naive_local()
}

/// The sidebar group for a meeting day: "Today", "Yesterday", a weekday and
/// date this year ("Mon, Oct 27"), or a full date before that.
pub fn group_label(day: NaiveDate, today: NaiveDate) -> String {
    if day == today {
        "Today".into()
    } else if today.pred_opt() == Some(day) {
        "Yesterday".into()
    } else if day.year() == today.year() {
        day.format("%a, %b %-d").to_string()
    } else {
        day.format("%b %-d, %Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("meeting-transcriber-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(s: &str) -> NaiveDateTime {
        meeting::parse_date(s).unwrap()
    }

    #[test]
    fn titles_are_sanitized_for_file_names() {
        assert_eq!(sanitize_title("Q4: plan / budget?"), "Q4 plan budget");
        assert_eq!(sanitize_title("  ..hidden\nname.  "), "hidden name");
        assert_eq!(sanitize_title("周会：产品 <评审>"), "周会：产品 评审");
        assert_eq!(sanitize_title("///"), "");
        assert_eq!(sanitize_title(&"长".repeat(200)).chars().count(), MAX_TITLE_CHARS);
        let date = at("2026-10-29T14:05");
        assert_eq!(file_stem(date, "Weekly sync"), "2026-10-29 14-05 Weekly sync");
        assert_eq!(file_stem(date, "客户 会议"), "2026-10-29 14-05 客户 会议");
        assert_eq!(file_stem(date, "?"), "2026-10-29 14-05");
    }

    #[test]
    fn colliding_names_get_numbered() {
        let dir = temp_dir("names");
        let stem = "2026-10-29 14-05 Sync";
        let first = unused_path(&dir, stem, "md", None);
        assert_eq!(first, dir.join("2026-10-29 14-05 Sync.md"));
        std::fs::write(&first, "").unwrap();
        let second = unused_path(&dir, stem, "md", None);
        assert_eq!(second, dir.join("2026-10-29 14-05 Sync (2).md"));
        std::fs::write(&second, "").unwrap();
        assert_eq!(unused_path(&dir, stem, "md", None), dir.join(format!("{stem} (3).md")));
        // The file being renamed keeps its name, also when only case changes.
        assert_eq!(unused_path(&dir, stem, "md", Some(&second)), second);
        assert_eq!(unused_path(&dir, stem, "md", Some(&first)), first);
        assert_eq!(
            unused_path(&dir, "2026-10-29 14-05 SYNC", "md", Some(&first)),
            dir.join("2026-10-29 14-05 SYNC.md")
        );
        // Other extensions do not collide.
        assert_eq!(unused_path(&dir, stem, "txt", None), dir.join(format!("{stem}.txt")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_give_dates_and_titles() {
        assert_eq!(parse_stamp("2026-10-29 14-05 Sync.md"), Some(at("2026-10-29T14:05")));
        assert_eq!(
            parse_stamp("Meeting 2026-10-29 14-05 (2).txt"),
            Some(at("2026-10-29T14:05"))
        );
        assert_eq!(parse_stamp("notes.md"), None);
        assert_eq!(parse_stamp("会议.md"), None);
        assert!(is_legacy_name("Meeting 2026-10-29 14-05.txt"));
        assert!(is_legacy_name("Meeting 2026-10-29 14-05 (2).txt"));
        assert!(!is_legacy_name("Meeting notes.txt"));
        assert!(!is_legacy_name("2026-10-29 14-05 Sync.txt"));
        assert_eq!(title_from_name("2026-10-29 14-05 Weekly sync.md"), "Weekly sync");
        assert_eq!(title_from_name("2026-10-29 14-05.md"), "Meeting");
        assert_eq!(title_from_name("Ideas.md"), "Ideas");
        assert_eq!(
            title_from_name("Meeting 2026-10-29 14-05.txt"),
            "Meeting 2026-10-29 14-05"
        );
    }

    #[test]
    fn listing_reads_meetings_and_old_transcripts() {
        let dir = temp_dir("list");
        let mut m = meeting::Meeting::new("Planning", at("2026-10-28T09:00"));
        m.tags = vec!["team".into()];
        std::fs::write(dir.join("2026-10-28 09-00 Planning.md"), m.to_markdown()).unwrap();
        std::fs::write(dir.join("Meeting 2026-10-29 14-05.txt"), "[00:01] Me: hi\n").unwrap();
        std::fs::write(dir.join("2026-10-27 08-00 No front.md"), "# hi\n").unwrap();
        std::fs::write(dir.join("other.txt"), "x").unwrap();
        std::fs::write(dir.join(".hidden.md"), "x").unwrap();
        std::fs::create_dir_all(dir.join("folder.md")).unwrap();

        let mut lib = Library::default();
        assert!(lib.scan(&dir));
        let found: Vec<_> = lib.entries.iter().map(|e| (e.title.as_str(), e.kind)).collect();
        assert_eq!(
            found,
            [
                ("Meeting 2026-10-29 14-05", Kind::Text),
                ("Planning", Kind::Markdown),
                ("No front", Kind::Markdown),
            ]
        );
        assert_eq!(lib.entries[1].tags, ["team"]);
        assert!(!lib.scan(&dir), "nothing changed");

        m.title = "Planning v2".into();
        std::fs::write(dir.join("2026-10-28 09-00 Planning.md"), m.to_markdown()).unwrap();
        assert!(lib.scan(&dir));
        assert_eq!(lib.entries[1].title, "Planning v2");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(lib.scan(&dir));
        assert!(lib.entries.is_empty());
    }

    #[test]
    fn search_matches_titles_and_tags() {
        let e = Entry {
            path: PathBuf::new(),
            kind: Kind::Markdown,
            title: "Weekly Sync 周会".into(),
            date: at("2026-10-29T14:05"),
            tags: vec!["Client".into(), "q4".into()],
        };
        assert!(e.matches(""));
        assert!(e.matches("sync"));
        assert!(e.matches("周会"));
        assert!(e.matches("client weekly"));
        assert!(e.matches("#cli"));
        assert!(!e.matches("#weekly"));
        assert!(!e.matches("sync budget"));
    }

    #[test]
    fn day_groups() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let day = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        assert_eq!(group_label(today, today), "Today");
        assert_eq!(group_label(day(10, 28), today), "Yesterday");
        assert_eq!(group_label(day(10, 26), today), "Mon, Oct 26");
        assert_eq!(
            group_label(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap(), today),
            "Dec 31, 2025"
        );
        let jan1 = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap();
        assert_eq!(
            group_label(NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(), jan1),
            "Yesterday"
        );
    }
}
