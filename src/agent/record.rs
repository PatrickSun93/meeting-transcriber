//! The record of a call: what each side said, the tools used and how long
//! each step of each turn took, saved as Markdown in the same format as a
//! meeting (frontmatter, notes, `**[mm:ss] Who:** text` lines).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};
use chrono::NaiveDateTime;

use crate::meeting::Meeting;
use crate::meetings;
use crate::transcript::{Line, Speaker};

pub const CALLER: &str = "Caller";
pub const AGENT: &str = "Agent";
pub const TOOL: &str = "Tool";

/// When each step of one turn happened.
#[derive(Debug, Clone)]
pub struct Marks {
    /// The pause that ended the caller's utterance was heard.
    pub ended: Instant,
    pub asr: Instant,
    pub first_token: Option<Instant>,
    pub first_sentence: Option<Instant>,
    pub first_sound: Option<Instant>,
}

/// One turn's steps, in milliseconds after `ended`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Latency {
    pub asr: u32,
    pub first_token: Option<u32>,
    pub first_sentence: Option<u32>,
    pub first_sound: Option<u32>,
}

impl Marks {
    pub fn latency(&self) -> Latency {
        let ms = |t: Instant| t.saturating_duration_since(self.ended).as_millis() as u32;
        Latency {
            asr: ms(self.asr),
            first_token: self.first_token.map(ms),
            first_sentence: self.first_sentence.map(ms),
            first_sound: self.first_sound.map(ms),
        }
    }
}

fn ms(v: Option<u32>) -> String {
    v.map_or("–".into(), |v| v.to_string())
}

impl Latency {
    /// One line for the terminal.
    pub fn summary(&self) -> String {
        format!(
            "ASR {} ms · LLM first token {} ms · first sentence to TTS {} ms · first sound {} ms (after the end of speech was detected)",
            self.asr,
            ms(self.first_token),
            ms(self.first_sentence),
            ms(self.first_sound)
        )
    }
}

#[derive(Debug, Clone)]
pub struct TurnStats {
    pub caller: String,
    pub latency: Latency,
    pub tools: Vec<String>,
    pub interrupted: bool,
}

#[derive(Debug)]
pub struct Record {
    pub started: NaiveDateTime,
    t0: Instant,
    /// What the notes say about the setup.
    pub setup: Vec<String>,
    lines: Vec<Line>,
    turns: Vec<TurnStats>,
    pub bookings: Vec<String>,
    path: Option<PathBuf>,
}

/// The median of the values there are.
pub fn median(values: impl Iterator<Item = u32>) -> Option<u32> {
    let mut v: Vec<u32> = values.collect();
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

impl Record {
    pub fn new(started: NaiveDateTime, t0: Instant, setup: Vec<String>) -> Self {
        Self {
            started,
            t0,
            setup,
            lines: Vec::new(),
            turns: Vec::new(),
            bookings: Vec::new(),
            path: None,
        }
    }

    /// Seconds since the call started.
    pub fn at(&self, t: Instant) -> f32 {
        t.saturating_duration_since(self.t0).as_secs_f32()
    }

    pub fn say(&mut self, who: &str, start: f32, text: &str) {
        let speaker = if who == CALLER { Speaker::Me } else { Speaker::Others };
        self.lines.push(Line {
            speaker,
            start,
            end: start,
            text: text.to_string(),
            language: None,
            who: Some(who.to_string()),
        });
    }

    pub fn turn(&mut self, stats: TurnStats) {
        self.turns.push(stats);
    }

    pub fn turns(&self) -> &[TurnStats] {
        &self.turns
    }

    pub fn to_meeting(&self, duration: u64) -> Meeting {
        let mut m = Meeting::new("Voice agent call", self.started);
        m.duration = duration;
        m.languages = vec!["en".into()];
        m.tags = vec!["voice-agent".into(), "demo".into()];
        let mut lines = self.lines.clone();
        lines.sort_by(|a, b| a.start.total_cmp(&b.start));
        m.lines = lines;
        m.notes = self.notes();
        m
    }

    fn notes(&self) -> String {
        let mut n = String::from(
            "Experimental voice agent demo (`meeting-transcriber agent`). The clinic, its times and \
             its bookings are made-up demo data kept in memory.\n\n",
        );
        for s in &self.setup {
            let _ = writeln!(n, "- {s}");
        }
        n.push_str("\n### Latency per turn\n\nMilliseconds after the end of the caller's speech was detected (a pause of the set length).\n\n");
        n.push_str("| # | Caller | ASR done | LLM first token | First sentence to TTS | First sound | Tools |\n");
        n.push_str("|---|---|---|---|---|---|---|\n");
        for (i, t) in self.turns.iter().enumerate() {
            let mut caller: String = t.caller.chars().take(40).collect();
            if t.caller.chars().count() > 40 {
                caller.push('…');
            }
            let _ = writeln!(
                n,
                "| {} | {} | {} | {} | {} | {}{} | {} |",
                i + 1,
                caller.replace('|', "/"),
                t.latency.asr,
                ms(t.latency.first_token),
                ms(t.latency.first_sentence),
                ms(t.latency.first_sound),
                if t.interrupted { " (interrupted)" } else { "" },
                if t.tools.is_empty() {
                    "–".into()
                } else {
                    t.tools.join(", ")
                },
            );
        }
        if !self.turns.is_empty() {
            let l = |f: fn(&Latency) -> Option<u32>| ms(median(self.turns.iter().filter_map(|t| f(&t.latency))));
            let _ = writeln!(
                n,
                "| median | | {} | {} | {} | {} | |",
                l(|x| Some(x.asr)),
                l(|x| x.first_token),
                l(|x| x.first_sentence),
                l(|x| x.first_sound)
            );
        }
        if !self.bookings.is_empty() {
            n.push_str("\n### Bookings (demo)\n\n");
            for b in &self.bookings {
                let _ = writeln!(n, "- {b}");
            }
        }
        n
    }

    /// Writes the record into `dir`, the same file each time.
    pub fn save(&mut self, dir: &Path, duration: u64) -> Result<PathBuf> {
        let path = match &self.path {
            Some(p) => p.clone(),
            None => {
                std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
                let stem = meetings::file_stem(self.started, "Voice agent");
                let p = meetings::unused_path(dir, &stem, "md", None);
                self.path = Some(p.clone());
                p
            }
        };
        std::fs::write(&path, self.to_meeting(duration).to_markdown())
            .with_context(|| format!("could not write {}", path.display()))?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn latency_is_counted_from_the_end_of_speech() {
        let t = Instant::now();
        let marks = Marks {
            ended: t,
            asr: t + Duration::from_millis(300),
            first_token: Some(t + Duration::from_millis(700)),
            first_sentence: Some(t + Duration::from_millis(900)),
            first_sound: None,
        };
        let l = marks.latency();
        assert_eq!(
            (l.asr, l.first_token, l.first_sentence, l.first_sound),
            (300, Some(700), Some(900), None)
        );
        assert!(l.summary().contains("first sound – ms"));
        assert_eq!(median([5, 1, 3].into_iter()), Some(3));
        assert_eq!(median(std::iter::empty()), None);
    }

    #[test]
    fn the_record_reads_back_as_a_meeting() {
        let t0 = Instant::now();
        let date = NaiveDateTime::parse_from_str("2026-11-12 20:30", "%Y-%m-%d %H:%M").unwrap();
        let mut r = Record::new(date, t0, vec!["LLM: ollama gemma4:12b".into()]);
        r.say(AGENT, 0.0, "Hi, thanks for calling.");
        r.say(CALLER, 3.2, "Anything open | Monday?");
        r.say(TOOL, 4.0, "check_availability {\"date\":\"2026-11-16\"}");
        r.say(AGENT, 4.5, "We have nine and nine thirty. —");
        r.turn(TurnStats {
            caller: "Anything open | Monday?".into(),
            latency: Latency {
                asr: 310,
                first_token: Some(650),
                first_sentence: Some(800),
                first_sound: Some(1200),
            },
            tools: vec!["check_availability".into()],
            interrupted: true,
        });
        r.bookings.push("MS-1041 Mon 2026-11-16 09:00, Jane Doe".into());
        let md = r.to_meeting(42).to_markdown();
        assert!(md.contains("**[00:03] Caller:** Anything open | Monday?"), "{md}");
        assert!(
            md.contains("| 1 | Anything open / Monday? | 310 | 650 | 800 | 1200 (interrupted) | check_availability |")
        );
        assert!(md.contains("| median | | 310 | 650 | 800 | 1200 | |"));
        assert!(md.contains("- MS-1041"));
        let back = Meeting::parse(&md, "x", date);
        assert_eq!(back.lines.len(), 4);
        assert_eq!(back.lines[1].label(), CALLER);
        assert_eq!(back.duration, 42);
        assert!(back.tags.contains(&"voice-agent".to_string()));
    }

    #[test]
    fn saving_twice_writes_one_file() {
        let dir = std::env::temp_dir().join(format!("agent-record-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let date = NaiveDateTime::parse_from_str("2026-11-12 20:30", "%Y-%m-%d %H:%M").unwrap();
        let mut r = Record::new(date, Instant::now(), Vec::new());
        r.say(AGENT, 0.0, "Hi.");
        let a = r.save(&dir, 1).unwrap();
        r.say(CALLER, 1.0, "Hello.");
        let b = r.save(&dir, 2).unwrap();
        assert_eq!(a, b);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert!(std::fs::read_to_string(&b).unwrap().contains("Hello."));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
