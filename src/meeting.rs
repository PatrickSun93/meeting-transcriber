//! A meeting as a Markdown file: YAML frontmatter, the user's notes and the
//! transcript. Written so it reads well in any editor (Obsidian included) and
//! still parses after it has been edited there.
//!
//! ```text
//! ---
//! title: "Weekly sync"
//! date: 2026-10-29T14:05
//! duration: 00:42:10
//! languages: [en, zh]
//! tags: [team, client]
//! ---
//!
//! # Weekly sync
//!
//! ## Notes
//!
//! free-form notes
//!
//! ## Transcript
//!
//! **[00:03] Me:** text
//! **[00:07] Others:** text
//! ```

use std::fmt::Write as _;
use std::io::BufRead;
use std::ops::Range;

use chrono::{NaiveDate, NaiveDateTime};

use crate::transcript::{Line, Speaker, format_time};

/// The boundary between notes and transcript. The last one in the file
/// counts, so notes may mention it freely.
const TRANSCRIPT_HEADING: &str = "## Transcript";
const NOTES_HEADING: &str = "## Notes";
/// Frontmatter keys this app reads and writes; any others are kept as they are.
const KNOWN_KEYS: &[&str] = &["title", "date", "duration", "languages", "tags"];
const DATE_FORMAT: &str = "%Y-%m-%dT%H:%M";

#[derive(Debug, Clone, PartialEq)]
pub struct Meeting {
    pub title: String,
    /// When the meeting started, local time.
    pub date: NaiveDateTime,
    /// Recorded time in seconds.
    pub duration: u64,
    /// Language codes in the order they were first heard.
    pub languages: Vec<String>,
    pub tags: Vec<String>,
    pub notes: String,
    pub lines: Vec<Line>,
    /// Frontmatter lines added elsewhere (e.g. Obsidian properties),
    /// written back unchanged after the keys above.
    pub extra_frontmatter: Vec<String>,
}

/// What the meeting list needs, read from the frontmatter alone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Header {
    pub title: Option<String>,
    pub date: Option<NaiveDateTime>,
    pub tags: Vec<String>,
}

impl Meeting {
    pub fn new(title: &str, date: NaiveDateTime) -> Self {
        Self {
            title: title.to_string(),
            date,
            duration: 0,
            languages: Vec::new(),
            tags: Vec::new(),
            notes: String::new(),
            lines: Vec::new(),
            extra_frontmatter: Vec::new(),
        }
    }

    /// The meeting's lines as plain text, the format of the old `.txt` files.
    pub fn to_text(&self) -> String {
        crate::transcript::lines_to_text(&self.lines)
    }

    pub fn to_markdown(&self) -> String {
        let title = one_line(&self.title);
        let mut out = String::from("---\n");
        let _ = writeln!(out, "title: {}", yaml_quote(&title));
        let _ = writeln!(out, "date: {}", self.date.format(DATE_FORMAT));
        let _ = writeln!(out, "duration: {}", format_duration(self.duration));
        let _ = writeln!(out, "languages: {}", yaml_list(&self.languages));
        let _ = writeln!(out, "tags: {}", yaml_list(&self.tags));
        for line in &self.extra_frontmatter {
            let _ = writeln!(out, "{line}");
        }
        out.push_str("---\n\n");
        let _ = writeln!(out, "# {title}\n");
        let _ = writeln!(out, "{NOTES_HEADING}\n");
        let notes = self.notes.trim_matches('\n');
        if !notes.is_empty() {
            let _ = writeln!(out, "{notes}\n");
        }
        let _ = writeln!(out, "{TRANSCRIPT_HEADING}\n");
        for l in &self.lines {
            let _ = writeln!(out, "**[{}] {}:** {}", format_time(l.start), l.speaker.label(), l.text);
        }
        out
    }

    /// Reads a meeting file. Never fails: missing parts get defaults, the
    /// title falls back to `fallback_title` and the date to `fallback_date`.
    pub fn parse(text: &str, fallback_title: &str, fallback_date: NaiveDateTime) -> Self {
        // Windows editors may start the file with a byte-order mark.
        let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
        let (front, body) = split_frontmatter(&text);
        let mut meeting = Self::new(fallback_title, fallback_date);
        let mut front_title = None;
        if let Some(front) = front {
            let fields = parse_frontmatter(front);
            meeting.extra_frontmatter = fields.extra;
            if let Some(date) = fields.header.date {
                meeting.date = date;
            }
            meeting.tags = fields.header.tags;
            meeting.languages = fields.languages;
            meeting.duration = fields.duration.unwrap_or(0);
            front_title = fields.header.title;
        }

        let body_lines: Vec<&str> = body.lines().collect();
        let boundary = body_lines.iter().rposition(|l| l.trim_end() == TRANSCRIPT_HEADING);
        let (before, transcript) = match boundary {
            Some(i) => (&body_lines[..i], &body_lines[i + 1..]),
            None => (&body_lines[..], &[][..]),
        };
        let (heading, mut notes) = split_notes(before);
        meeting.title = front_title.or(heading).unwrap_or_else(|| fallback_title.to_string());
        // Text typed between the heading and the first line has no line to
        // join; it moves up to the notes rather than being lost on save.
        let first_line = transcript
            .iter()
            .position(|l| parse_transcript_line(l).is_some())
            .unwrap_or(transcript.len());
        let stray = transcript[..first_line].join("\n");
        let stray = stray.trim_matches('\n');
        if !stray.trim().is_empty() {
            if !notes.is_empty() {
                notes.push_str("\n\n");
            }
            notes.push_str(stray);
        }
        meeting.notes = notes;
        meeting.lines = parse_lines(transcript[first_line..].iter().copied());
        // Files edited elsewhere may lack a duration; the last line still
        // bounds it, which matters when recording continues.
        if let Some(last) = meeting.lines.last() {
            meeting.duration = meeting.duration.max(last.start.ceil() as u64);
        }
        meeting
    }
}

/// Splits `---` frontmatter from the body. None when the file has none
/// (or it is never closed).
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, text)
}

struct Frontmatter {
    header: Header,
    duration: Option<u64>,
    languages: Vec<String>,
    extra: Vec<String>,
}

fn parse_frontmatter(front: &str) -> Frontmatter {
    let mut fm = Frontmatter {
        header: Header::default(),
        duration: None,
        languages: Vec::new(),
        extra: Vec::new(),
    };
    let lines: Vec<&str> = front.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let key_value = (!line.starts_with([' ', '\t', '-', '#']))
            .then(|| line.split_once(':'))
            .flatten()
            .map(|(k, v)| (k.trim(), v.trim()));
        let Some((key, value)) = key_value.filter(|(k, _)| KNOWN_KEYS.contains(k)) else {
            fm.extra.push(line.to_string());
            continue;
        };
        // Obsidian writes lists as indented "- item" lines below the key.
        let mut items = Vec::new();
        while i < lines.len() && is_block_item(lines[i]) {
            let item = lines[i].trim_start().trim_start_matches('-').trim();
            items.push(yaml_unquote(item));
            i += 1;
        }
        match key {
            "title" => fm.header.title = Some(yaml_unquote(value)).filter(|t| !t.is_empty()),
            "date" => fm.header.date = parse_date(&yaml_unquote(value)),
            "duration" => fm.duration = parse_duration(&yaml_unquote(value)),
            "tags" => fm.header.tags = list_values(value, items),
            "languages" => fm.languages = list_values(value, items),
            _ => unreachable!("only known keys get here"),
        }
    }
    fm
}

fn is_block_item(line: &str) -> bool {
    let t = line.trim_start();
    t == "-" || t.starts_with("- ")
}

/// A list written as `[a, b]`, as block items, or as a bare `a, b`.
fn list_values(value: &str, block_items: Vec<String>) -> Vec<String> {
    let values = if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        split_flow_list(inner)
    } else if value.is_empty() {
        block_items
    } else {
        value.split(',').map(|v| yaml_unquote(v.trim())).collect()
    };
    values
        .into_iter()
        .map(|v| v.trim().trim_start_matches('#').to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Splits the inside of `[a, "b, c", 'd']` at commas outside quotes.
fn split_flow_list(inner: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for c in inner.chars() {
        match quote {
            Some(q) => {
                current.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if c == ',' => items.push(std::mem::take(&mut current)),
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                }
                current.push(c);
            }
        }
    }
    items.push(current);
    items.iter().map(|i| yaml_unquote(i.trim())).collect()
}

/// The title heading and the notes from the part before the transcript.
/// Everything except the `# Title` line and a leading `## Notes` heading is
/// notes, so text added anywhere above the transcript is kept.
fn split_notes(lines: &[&str]) -> (Option<String>, String) {
    let mut rest = lines;
    let mut heading = None;
    let first = rest.iter().position(|l| !l.trim().is_empty());
    if let Some(i) = first
        && let Some(title) = rest[i].strip_prefix("# ")
    {
        heading = Some(title.trim().to_string()).filter(|t| !t.is_empty());
        rest = &rest[i + 1..];
    }
    if let Some(i) = rest.iter().position(|l| !l.trim().is_empty())
        && rest[i].trim_end() == NOTES_HEADING
    {
        rest = &rest[i + 1..];
    }
    let notes = rest.join("\n");
    (heading, notes.trim_matches('\n').to_string())
}

/// Transcript lines. Text that is not a `[time] Speaker:` line (wrapped or
/// typed in by hand) is joined onto the line before it, so it is kept.
fn parse_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    for raw in lines {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        if let Some(line) = parse_transcript_line(raw) {
            out.push(line);
        } else if let Some(last) = out.last_mut() {
            if !last.text.is_empty() {
                last.text.push(' ');
            }
            last.text.push_str(raw);
        }
    }
    out
}

/// One line: `**[00:03] Me:** text`, or the plain `[00:03] Me: text` of
/// the old `.txt` files. Also accepts a leading list dash.
pub fn parse_transcript_line(raw: &str) -> Option<Line> {
    let s = raw.trim().strip_prefix("- ").unwrap_or(raw.trim());
    let s = s.strip_prefix("**").unwrap_or(s);
    let rest = s.strip_prefix('[')?;
    let (time, rest) = rest.split_once(']')?;
    let start = parse_time(time)?;
    let (label, text) = rest.trim_start().split_once(':')?;
    let speaker = match label.trim() {
        "Me" => Speaker::Me,
        "Others" => Speaker::Others,
        _ => return None,
    };
    let text = text.strip_prefix("**").unwrap_or(text).trim();
    Some(Line {
        speaker,
        start,
        end: start,
        text: text.to_string(),
        language: None,
    })
}

/// Lines of an old plain-text transcript (`[mm:ss] Me: text`).
pub fn parse_legacy_text(text: &str) -> Vec<Line> {
    parse_lines(text.lines())
}

/// `03:07` or `1:02:05` (as written by `format_time`) in seconds.
pub fn parse_time(s: &str) -> Option<f32> {
    let parts: Vec<u64> = s
        .trim()
        .split(':')
        .map(|p| (!p.is_empty() && p.len() <= 3).then(|| p.parse().ok()).flatten())
        .collect::<Option<_>>()?;
    let seconds = match parts[..] {
        [m, s] if s < 60 => m * 60 + s,
        [h, m, s] if m < 60 && s < 60 => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(seconds as f32)
}

/// Always `hh:mm:ss`, e.g. `00:42:10`.
pub fn format_duration(seconds: u64) -> String {
    format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

fn parse_duration(s: &str) -> Option<u64> {
    if let Ok(seconds) = s.trim().parse::<u64>() {
        return Some(seconds);
    }
    parse_time(s).map(|t| t as u64)
}

/// `2026-10-29T14:05`, also with seconds, a space instead of `T`, or no time.
pub fn parse_date(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    [
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d %H:%M:%S",
    ]
    .iter()
    .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
    .or_else(|| {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
    })
}

/// Reads only the frontmatter of a meeting file, stopping at its end, so
/// listing a folder of long transcripts stays cheap.
pub fn read_header(reader: impl BufRead) -> Header {
    const MAX_LINES: usize = 500;
    let mut lines = reader.lines().map_while(Result::ok).take(MAX_LINES);
    if lines
        .next()
        .is_none_or(|first| first.trim_start_matches('\u{feff}').trim_end() != "---")
    {
        return Header::default();
    }
    let mut front = String::new();
    for line in lines {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            return parse_frontmatter(&front).header;
        }
        front.push_str(&line);
        front.push('\n');
    }
    Header::default()
}

/// Parses frontmatter text (without the `---` fences).
pub fn parse_header(front: &str) -> Header {
    parse_frontmatter(front).header
}

/// Titles are single lines: line breaks become spaces.
pub fn one_line(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A YAML double-quoted string.
fn yaml_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `[a, b]`, quoting items that YAML would otherwise misread.
fn yaml_list(items: &[String]) -> String {
    let items: Vec<String> = items
        .iter()
        .map(|i| {
            let plain = !i.is_empty()
                && i.trim() == i
                && !i.contains([',', '[', ']', '{', '}', '"', '\'', ':', '#', '\\'])
                && !i.chars().any(char::is_control);
            if plain { i.clone() } else { yaml_quote(i) }
        })
        .collect();
    format!("[{}]", items.join(", "))
}

/// A YAML scalar: double-quoted (with escapes), single-quoted, or plain.
fn yaml_unquote(s: &str) -> String {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
        && s.len() >= 2
    {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                }
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        }
        return out;
    }
    if let Some(inner) = s.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))
        && s.len() >= 2
    {
        return inner.replace("''", "'");
    }
    s.to_string()
}

/// `known` plus the languages the lines were detected in, in first-heard order.
pub fn merge_languages(known: &[String], lines: &[Line]) -> Vec<String> {
    let mut out = known.to_vec();
    for code in lines.iter().filter_map(|l| l.language.as_deref()) {
        if !out.iter().any(|c| c == code) {
            out.push(code.to_string());
        }
    }
    out
}

/// Tags typed as `team, client` (a leading `#` is dropped).
pub fn parse_tags_input(input: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in input.split(',') {
        let tag = tag.trim().trim_start_matches('#').trim();
        if !tag.is_empty() && !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_string());
        }
    }
    tags
}

/// Byte ranges of `query` in `text`, ignoring case, without overlaps.
pub fn find_matches(text: &str, query: &str) -> Vec<Range<usize>> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut from = 0;
    for (start, _) in text.char_indices() {
        if start < from {
            continue;
        }
        if let Some(len) = match_len_at(&text[start..], &query) {
            out.push(start..start + len);
            from = start + len;
        }
    }
    out
}

/// Bytes of `text` that match the lower-cased `query` at its start.
fn match_len_at(text: &str, query: &[char]) -> Option<usize> {
    let mut q = query.iter();
    let mut want = q.next();
    for (i, c) in text.char_indices() {
        for lower in c.to_lowercase() {
            match want {
                Some(&w) if w == lower => want = q.next(),
                _ => return None,
            }
        }
        if want.is_none() {
            return Some(i + c.len_utf8());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDateTime {
        parse_date(s).unwrap()
    }

    fn line(speaker: Speaker, start: f32, text: &str) -> Line {
        Line {
            speaker,
            start,
            end: start,
            text: text.into(),
            language: None,
        }
    }

    fn sample() -> Meeting {
        Meeting {
            title: "Weekly \"sync\" \\ planning".into(),
            date: date("2026-10-29T14:05"),
            duration: 3600 + 2 * 60 + 10,
            languages: vec!["en".into(), "zh".into()],
            tags: vec!["team".into(), "client a".into(), "q4, 2026".into()],
            notes: "- ship on Friday\n\n## Action items\n- Alex: send deck".into(),
            lines: vec![
                line(Speaker::Me, 3.0, "Hi, can you hear me?"),
                line(Speaker::Others, 7.0, "Yes: loud and clear. **Great**"),
                line(Speaker::Me, 3725.0, "我们周五发布。"),
            ],
            extra_frontmatter: Vec::new(),
        }
    }

    fn parse(text: &str) -> Meeting {
        Meeting::parse(text, "Fallback", date("2000-01-01"))
    }

    #[test]
    fn markdown_round_trips() {
        let m = sample();
        let md = m.to_markdown();
        assert!(md.starts_with("---\ntitle: \"Weekly \\\"sync\\\" \\\\ planning\"\ndate: 2026-10-29T14:05\n"));
        assert!(md.contains("duration: 01:02:10\nlanguages: [en, zh]\ntags: [team, client a, \"q4, 2026\"]\n---\n"));
        assert!(md.contains("\n## Transcript\n\n**[00:03] Me:** Hi, can you hear me?\n**[00:07] Others:** Yes"));
        assert!(md.contains("**[1:02:05] Me:** 我们周五发布。\n"));
        let back = parse(&md);
        assert_eq!(back, m);
        assert_eq!(parse(&back.to_markdown()).to_markdown(), md);
    }

    #[test]
    fn empty_meeting_round_trips() {
        let m = Meeting::new("会议 周会", date("2026-01-02T09:30"));
        let md = m.to_markdown();
        assert!(md.contains("# 会议 周会\n\n## Notes\n\n## Transcript\n"));
        assert_eq!(parse(&md), m);
    }

    #[test]
    fn notes_may_mention_the_transcript_heading() {
        let mut m = sample();
        m.notes = "## Transcript\nthis is still a note".into();
        assert_eq!(parse(&m.to_markdown()).notes, m.notes);
    }

    #[test]
    fn external_edits_are_tolerated() {
        // Obsidian-style properties: block lists, extra keys, unquoted title.
        let text = "---\r\n\
            title: Planning 'Q4'\r\n\
            date: 2026-10-29 14:05\r\n\
            aliases:\r\n  - plan\r\n\
            tags:\r\n  - team\r\n  - \"#client\"\r\n\
            status: done\r\n\
            ---\r\n\
            # Planning 'Q4'\r\n\
            \r\n\
            Typed above the notes heading.\r\n\
            ## Notes\r\n\
            Edited in Obsidian.\r\n\
            \r\n\
            ## Transcript\r\n\
            \r\n\
            **[00:03] Me:** Hello\r\n\
            wrapped by hand\r\n\
            \r\n\
            [00:09] Others: plain line\r\n\
            - **[01:00] Others:** a list item\r\n";
        let m = parse(text);
        assert_eq!(m.title, "Planning 'Q4'");
        assert_eq!(m.date, date("2026-10-29T14:05"));
        assert_eq!(m.tags, ["team", "client"]);
        assert_eq!(m.extra_frontmatter, ["aliases:", "  - plan", "status: done"]);
        assert_eq!(m.notes, "Typed above the notes heading.\n## Notes\nEdited in Obsidian.");
        let texts: Vec<_> = m.lines.iter().map(|l| (l.start, l.speaker, l.text.as_str())).collect();
        assert_eq!(
            texts,
            [
                (3.0, Speaker::Me, "Hello wrapped by hand"),
                (9.0, Speaker::Others, "plain line"),
                (60.0, Speaker::Others, "a list item"),
            ]
        );
        assert_eq!(m.duration, 60);
        // Unknown keys survive a save.
        let saved = m.to_markdown();
        assert!(saved.contains("tags: [team, client]\naliases:\n  - plan\nstatus: done\n---\n"));
        assert_eq!(parse(&saved), m);
    }

    #[test]
    fn text_above_the_first_line_is_kept() {
        let text =
            "---\ntitle: x\n---\n# x\n\n## Notes\n\nmine\n\n## Transcript\n\nTyped by hand.\n\n**[00:01] Me:** a\n";
        let m = parse(text);
        assert_eq!(m.notes, "mine\n\nTyped by hand.");
        assert_eq!(m.lines.len(), 1);
        assert_eq!(m.lines[0].text, "a");
        // Saved and read again, nothing moves further.
        assert_eq!(parse(&m.to_markdown()), m);
        // No notes: the stray text becomes the notes.
        let m = parse("## Transcript\nonly text\n");
        assert_eq!((m.notes.as_str(), m.lines.len()), ("only text", 0));
    }

    #[test]
    fn byte_order_mark_is_ignored() {
        let md = format!("\u{feff}{}", sample().to_markdown());
        assert_eq!(parse(&md), sample());
        assert_eq!(read_header(md.as_bytes()).title, sample().title.into());
    }

    #[test]
    fn missing_parts_fall_back() {
        let m = parse("Just some text\n");
        assert_eq!(m.title, "Fallback");
        assert_eq!(m.date, date("2000-01-01"));
        assert_eq!(m.notes, "Just some text");
        assert!(m.lines.is_empty());
        // The H1 is the title when the frontmatter has none.
        let m = parse("---\ntags: x\n---\n# From heading\n\n## Transcript\n**[00:01] Me:** a\n");
        assert_eq!(m.title, "From heading");
        assert_eq!(m.tags, ["x"]);
        assert_eq!(m.notes, "");
        assert_eq!(m.lines.len(), 1);
        // Unclosed frontmatter is treated as body text.
        assert_eq!(parse("---\ntitle: x\n").title, "Fallback");
    }

    #[test]
    fn header_reads_only_frontmatter() {
        let md = sample().to_markdown();
        let h = read_header(md.as_bytes());
        assert_eq!(h.title.as_deref(), Some("Weekly \"sync\" \\ planning"));
        assert_eq!(h.date, Some(date("2026-10-29T14:05")));
        assert_eq!(h.tags, ["team", "client a", "q4, 2026"]);
        assert_eq!(read_header("no frontmatter".as_bytes()), Header::default());
        assert_eq!(parse_header("title: '它''s'").title.as_deref(), Some("它's"));
    }

    #[test]
    fn legacy_text_parses() {
        let text = "[00:03] Me: Hi.\n[1:02:05] Others: Bye: see you.\n\nnot a line\n";
        let lines = parse_legacy_text(text);
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].speaker, lines[0].start), (Speaker::Me, 3.0));
        assert_eq!(lines[1].text, "Bye: see you. not a line");
        assert_eq!(lines[1].start, 3725.0);
        // And back to the same text.
        let mut m = Meeting::new("x", date("2026-01-01"));
        m.lines = parse_legacy_text("[00:03] Me: Hi.\n[1:02:05] Others: Bye.\n");
        assert_eq!(m.to_text(), "[00:03] Me: Hi.\n[1:02:05] Others: Bye.\n");
    }

    #[test]
    fn times_and_durations() {
        assert_eq!(parse_time("00:03"), Some(3.0));
        assert_eq!(parse_time("1:02:05"), Some(3725.0));
        assert_eq!(parse_time("125:00"), Some(7500.0));
        assert_eq!(parse_time("00:61"), None);
        assert_eq!(parse_time("ab:cd"), None);
        assert_eq!(parse_time(""), None);
        assert_eq!(format_duration(42 * 60 + 10), "00:42:10");
        assert_eq!(parse_duration("00:42:10"), Some(2530));
        assert_eq!(parse_duration("90"), Some(90));
        assert_eq!(parse_date("2026-10-29"), Some(date("2026-10-29T00:00")));
        assert_eq!(parse_date("2026-10-29T14:05:59"), parse_date("2026-10-29 14:05:59"));
        assert_eq!(parse_date("yesterday"), None);
    }

    #[test]
    fn languages_are_collected_once() {
        let mut lines = vec![
            line(Speaker::Me, 0.0, "a"),
            line(Speaker::Others, 1.0, "b"),
            line(Speaker::Me, 2.0, "c"),
        ];
        lines[0].language = Some("zh".into());
        lines[1].language = Some("en".into());
        lines[2].language = Some("zh".into());
        assert_eq!(merge_languages(&["en".into()], &lines), ["en", "zh"]);
        assert_eq!(merge_languages(&[], &lines), ["zh", "en"]);
    }

    #[test]
    fn tags_input() {
        assert_eq!(
            parse_tags_input(" team, #client ,, team,客户 "),
            ["team", "client", "客户"]
        );
        assert!(parse_tags_input("  ").is_empty());
    }

    #[test]
    fn titles_are_one_line() {
        let mut m = Meeting::new("a\nb\t c", date("2026-01-01"));
        assert_eq!(one_line(&m.title), "a b c");
        m.title = one_line(&m.title);
        assert_eq!(parse(&m.to_markdown()).title, "a b c");
    }

    #[test]
    fn find_matches_ignores_case() {
        assert_eq!(find_matches("Ship it, SHIP it", "ship"), [0..4, 9..13]);
        assert_eq!(find_matches("aaaa", "aa"), [0..2, 2..4]);
        assert_eq!(find_matches("我们周五发布，周五见", "周五"), [6..12, 21..27]);
        assert!(find_matches("abc", "").is_empty());
        assert!(find_matches("abc", "abcd").is_empty());
        // Ranges are byte ranges of the original text.
        let text = "Ärger mit ÄRGER";
        let found = find_matches(text, "ärger");
        assert_eq!(found.len(), 2);
        assert_eq!(&text[found[1].clone()], "ÄRGER");
    }
}
