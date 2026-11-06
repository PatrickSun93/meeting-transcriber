//! Where screenshots go and how notes link them: a meeting's pictures in a
//! folder beside its file ("<file name> files"), named by the meeting's
//! clock; others in a "Screenshots" folder in the meetings folder. The note
//! is a Markdown image link, which Obsidian and other Markdown apps show.

use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;

/// The folder in the meetings folder for screenshots taken with no meeting open.
pub const LOOSE_DIR: &str = "Screenshots";

/// When a screenshot was taken, as its name tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// This far into the recording meeting, in seconds.
    Meeting(u64),
    /// The time of day, in an open meeting that is not recording.
    Clock(NaiveDateTime),
    /// Date and time, with no meeting open.
    Loose(NaiveDateTime),
}

impl Moment {
    /// The file name without its extension, e.g. "Screenshot 12-03".
    pub fn stem(self) -> String {
        format!("Screenshot {}", self.when('-'))
    }

    /// What the note shows, e.g. "Screenshot 12:03".
    pub fn label(self) -> String {
        format!("Screenshot {}", self.when(':'))
    }

    fn when(self, sep: char) -> String {
        match self {
            Moment::Meeting(s) if s >= 3600 => format!("{}{sep}{:02}{sep}{:02}", s / 3600, s / 60 % 60, s % 60),
            Moment::Meeting(s) => format!("{:02}{sep}{:02}", s / 60, s % 60),
            Moment::Clock(t) => t.format(&format!("%H{sep}%M{sep}%S")).to_string(),
            Moment::Loose(t) => t.format("%Y-%m-%d %H-%M-%S").to_string(),
        }
    }
}

/// The folder for a meeting's pictures: beside its file, named after it.
pub fn meeting_dir(meeting_file: &Path) -> PathBuf {
    let stem = meeting_file.file_stem().unwrap_or_default().to_string_lossy();
    meeting_file.with_file_name(format!("{stem} files"))
}

/// `dir/stem.png`, or with " 2", " 3", … added when that is taken: on
/// disk, or in `pending` (being written).
pub fn unused_png(dir: &Path, stem: &str, pending: &[PathBuf]) -> PathBuf {
    let free = |p: &PathBuf| !p.exists() && !pending.contains(p);
    std::iter::once(dir.join(format!("{stem}.png")))
        .chain((2..).map(|n| dir.join(format!("{stem} {n}.png"))))
        .find(free)
        .expect("some number is free")
}

/// `path` as a link from a file in `from_dir`: relative, with `/`, and the
/// characters Markdown links trip over percent-encoded (spaces, brackets).
/// None when it is not inside `from_dir`.
pub fn relative_link(from_dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(from_dir).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| encode_part(&c.as_os_str().to_string_lossy()))
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn encode_part(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    for c in part.chars() {
        match c {
            ' ' | '%' | '(' | ')' | '[' | ']' | '<' | '>' | '#' | '?' | '^' | '|' | '"' => {
                out += &format!("%{:02X}", c as u32)
            }
            c => out.push(c),
        }
    }
    out
}

/// Undoes the percent-encoding of a link (as `relative_link` writes it, or
/// a browser would); bytes that do not decode are kept as written.
pub fn decode_link(link: &str) -> String {
    let bytes = link.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = link.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| link.to_string())
}

/// The note that shows a picture: `![label](link)`.
pub fn image_note(label: &str, link: &str) -> String {
    format!("![{}]({link})", label.replace(['[', ']'], ""))
}

/// `notes` with `line` added as a line of its own at the end.
pub fn append_line(notes: &str, line: &str) -> String {
    let kept = notes.trim_end_matches([' ', '\t', '\r', '\n']);
    if kept.is_empty() {
        line.to_string()
    } else {
        format!("{kept}\n{line}")
    }
}

/// The targets of the image links in `notes` (`![…](target)`), in order:
/// local files only, not web addresses.
pub fn image_links(notes: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = notes;
    while let Some(start) = rest.find("![") {
        rest = &rest[start + 2..];
        let Some(close) = rest.find("](") else { break };
        if rest[..close].contains('\n') {
            continue;
        }
        let after = &rest[close + 2..];
        let Some(end) = after.find(')') else { break };
        let target = after[..end].trim();
        // `![x](<path with spaces>)` and `![x](path "title")`.
        let target = match target.strip_prefix('<') {
            Some(t) => t.split('>').next().unwrap_or(t),
            None => target.split(" \"").next().unwrap_or(target),
        };
        if !target.is_empty() && !target.contains('\n') && !target.contains("://") {
            links.push(target.to_string());
        }
        rest = &after[end..];
    }
    links
}

/// The file a link in a meeting's notes points at: relative to the
/// meeting's folder (`dir`), decoded.
pub fn link_path(dir: &Path, link: &str) -> PathBuf {
    let decoded = decode_link(link);
    let path = Path::new(&decoded);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        dir.join(decoded.split('/').collect::<PathBuf>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn names_follow_the_meeting_clock_or_the_time_of_day() {
        assert_eq!(Moment::Meeting(723).stem(), "Screenshot 12-03");
        assert_eq!(Moment::Meeting(723).label(), "Screenshot 12:03");
        assert_eq!(Moment::Meeting(5).stem(), "Screenshot 00-05");
        assert_eq!(Moment::Meeting(3723).label(), "Screenshot 1:02:03");
        let t = at("2026-11-05 14:05:09");
        assert_eq!(Moment::Clock(t).stem(), "Screenshot 14-05-09");
        assert_eq!(Moment::Clock(t).label(), "Screenshot 14:05:09");
        assert_eq!(Moment::Loose(t).stem(), "Screenshot 2026-11-05 14-05-09");
    }

    #[test]
    fn a_meetings_pictures_go_beside_it() {
        let md = Path::new("/notes/2026-11-05 14-05 Weekly sync.md");
        assert_eq!(
            meeting_dir(md),
            PathBuf::from("/notes/2026-11-05 14-05 Weekly sync files")
        );
        let dir = std::env::temp_dir().join(format!("meeting-transcriber-test-{}-png", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            unused_png(&dir, "Screenshot 12-03", &[]),
            dir.join("Screenshot 12-03.png")
        );
        std::fs::write(dir.join("Screenshot 12-03.png"), b"").unwrap();
        std::fs::write(dir.join("Screenshot 12-03 2.png"), b"").unwrap();
        assert_eq!(
            unused_png(&dir, "Screenshot 12-03", &[]),
            dir.join("Screenshot 12-03 3.png")
        );
        // One still being written counts as taken.
        let pending = [dir.join("Screenshot 12-03 3.png")];
        assert_eq!(
            unused_png(&dir, "Screenshot 12-03", &pending),
            dir.join("Screenshot 12-03 4.png")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_are_relative_and_encoded() {
        let dir = Path::new("/notes");
        let png = Path::new("/notes/2026-11-05 Sync (Q&A) files/Screenshot 12-03.png");
        let link = relative_link(dir, png).unwrap();
        assert_eq!(link, "2026-11-05%20Sync%20%28Q&A%29%20files/Screenshot%2012-03.png");
        assert_eq!(decode_link(&link), "2026-11-05 Sync (Q&A) files/Screenshot 12-03.png");
        assert_eq!(link_path(dir, &link), png);
        // Other scripts are kept as they are; Markdown apps read them.
        assert_eq!(
            relative_link(dir, Path::new("/notes/周会 files/a.png")).unwrap(),
            "周会%20files/a.png"
        );
        assert_eq!(relative_link(dir, Path::new("/elsewhere/a.png")), None);
        assert_eq!(decode_link("100%"), "100%");
        assert_eq!(decode_link("%E5%91%A8"), "周");
        if cfg!(unix) {
            assert_eq!(link_path(dir, "/abs/a.png"), PathBuf::from("/abs/a.png"));
        }
    }

    #[test]
    fn notes_get_the_picture_on_a_line_of_its_own() {
        let note = image_note("Screenshot 12:03", "a%20files/b.png");
        assert_eq!(note, "![Screenshot 12:03](a%20files/b.png)");
        assert_eq!(append_line("", &note), note);
        assert_eq!(append_line("  \n\n", &note), note);
        assert_eq!(append_line("- point", &note), format!("- point\n{note}"));
        assert_eq!(append_line("- point\n\n", &note), format!("- point\n{note}"));
        assert_eq!(image_note("a [b]", "x"), "![a b](x)");
    }

    #[test]
    fn image_links_are_found_in_notes() {
        let notes = "Intro ![one](a%20files/1.png) and\n\
                     ![two](<b files/2.png>)\n\
                     ![web](https://example.com/x.png)\n\
                     [not an image](c.png) ![titled](d.png \"Title\")\n\
                     ![broken\n](e.png) ![open](f.png";
        assert_eq!(image_links(notes), ["a%20files/1.png", "b files/2.png", "d.png"]);
        assert!(image_links("no pictures").is_empty());
    }
}
