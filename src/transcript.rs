//! The running transcript: final lines, live previews and the echo filter.

use std::collections::HashSet;
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Speaker {
    /// Your microphone.
    Me,
    /// Everything the Mac plays: all other meeting participants.
    Others,
}

impl Speaker {
    pub fn label(self) -> &'static str {
        match self {
            Speaker::Me => "Me",
            Speaker::Others => "Others",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Speaker::Me => 0,
            Speaker::Others => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub speaker: Speaker,
    pub start: f32,
    pub end: f32,
    pub text: String,
    /// Detected language code, e.g. "en" or "zh".
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub utterance: u64,
    pub start: f32,
    pub text: String,
}

/// Whisper invents these for silence, music or noise.
const HALLUCINATIONS: &[&str] = &[
    "thank you",
    "thank you.",
    "thanks for watching",
    "thanks for watching!",
    "thank you for watching",
    "thank you for watching.",
    "you",
    "bye.",
    "字幕由amara.org社区提供",
    "请不吝点赞 订阅 转发 打赏支持明镜与点点栏目",
    "谢谢观看",
    "谢谢大家",
    "ご視聴ありがとうございました",
    "sous-titrage st' 501",
    "untertitel der amara.org-community",
];

/// Cleans Whisper output; returns None when nothing real was said.
pub fn clean_text(raw: &str) -> Option<String> {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let bracketed = |open: char, close: char| text.starts_with(open) && text.ends_with(close);
    if bracketed('[', ']') || bracketed('(', ')') || bracketed('*', '*') || bracketed('♪', '♪') {
        return None;
    }
    let lower = text.to_lowercase();
    if HALLUCINATIONS.contains(&lower.as_str()) {
        return None;
    }
    // Lines made only of punctuation or music notes.
    if !text.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some(text.to_string())
}

/// Lower-cased letter/digit pairs. Works the same for languages without
/// spaces (Chinese, Japanese) as for English.
fn bigrams(s: &str) -> HashSet<(char, char)> {
    let chars: Vec<char> = s
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Share of `part`'s character pairs that also occur in `whole`, 0..=1.
pub fn containment(part: &str, whole: &str) -> f32 {
    let (p, w) = (bigrams(part), bigrams(whole));
    if p.is_empty() {
        return 0.0;
    }
    p.intersection(&w).count() as f32 / p.len() as f32
}

/// When the meeting plays through speakers, the mic hears the other people
/// too and Whisper writes their words twice: once as Others, once as Me
/// (often split into smaller pieces). A Me line whose words are contained in
/// an Others line from the same moment is echo.
pub struct EchoFilter {
    pub enabled: bool,
    /// Allowed gap between the two lines, in seconds.
    pub window_seconds: f32,
    /// Share of the Me line that must appear in the Others line.
    pub threshold: f32,
    /// Shorter Me lines (e.g. "Yes.") are never treated as echo.
    pub min_pairs: usize,
}

impl Default for EchoFilter {
    fn default() -> Self {
        Self {
            enabled: true,
            window_seconds: 1.5,
            threshold: 0.8,
            min_pairs: 4,
        }
    }
}

impl EchoFilter {
    pub fn is_echo(&self, me: &Line, others: &Line) -> bool {
        self.enabled
            && me.speaker == Speaker::Me
            && others.speaker == Speaker::Others
            && me.start < others.end + self.window_seconds
            && others.start < me.end + self.window_seconds
            && bigrams(&me.text).len() >= self.min_pairs
            && containment(&me.text, &others.text) >= self.threshold
    }
}

#[derive(Default)]
pub struct Transcript {
    lines: Vec<Line>,
    previews: [Option<Preview>; 2],
    /// Last utterance per speaker that produced a final result.
    finished: [Option<u64>; 2],
    pub echo: EchoFilter,
    /// Number of Me lines removed as echo, shown in the UI.
    pub echo_removed: usize,
}

impl Transcript {
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub fn preview(&self, speaker: Speaker) -> Option<&Preview> {
        self.previews[speaker.index()].as_ref()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.previews = [None, None];
        self.finished = [None, None];
        self.echo_removed = 0;
    }

    /// Shows live text for an utterance still in progress.
    pub fn set_preview(&mut self, speaker: Speaker, utterance: u64, start: f32, text: Option<String>) {
        // A late partial must not bring back an utterance that already finished.
        if self.finished[speaker.index()].is_some_and(|f| f >= utterance) {
            return;
        }
        let slot = &mut self.previews[speaker.index()];
        if slot.as_ref().is_some_and(|p| p.utterance > utterance) {
            return;
        }
        *slot = text.map(|text| Preview { utterance, start, text });

        // Others' live text can already show that a Me line was echo.
        if speaker == Speaker::Others
            && let Some(others) = self.others_in_progress()
        {
            let before = self.lines.len();
            let echo = &self.echo;
            self.lines.retain(|me| !echo.is_echo(me, &others));
            self.echo_removed += before - self.lines.len();
        }
    }

    /// The Others utterance still in progress, as a line with an open end.
    fn others_in_progress(&self) -> Option<Line> {
        self.previews[Speaker::Others.index()].as_ref().map(|p| Line {
            speaker: Speaker::Others,
            start: p.start,
            end: f32::MAX,
            text: p.text.clone(),
            language: None,
        })
    }

    /// Marks an utterance as done (also when it produced no text) and
    /// removes its live preview.
    pub fn end_utterance(&mut self, speaker: Speaker, utterance: u64) {
        let done = &mut self.finished[speaker.index()];
        *done = Some(done.map_or(utterance, |d| d.max(utterance)));
        let slot = &mut self.previews[speaker.index()];
        if slot.as_ref().is_some_and(|p| p.utterance <= utterance) {
            *slot = None;
        }
    }

    /// Adds a finished line in time order. Returns false if it was dropped as echo.
    pub fn add_final(&mut self, utterance: u64, line: Line) -> bool {
        self.end_utterance(line.speaker, utterance);

        match line.speaker {
            Speaker::Me => {
                let in_progress = self.others_in_progress();
                if self
                    .lines
                    .iter()
                    .chain(&in_progress)
                    .any(|o| self.echo.is_echo(&line, o))
                {
                    self.echo_removed += 1;
                    return false;
                }
            }
            Speaker::Others => {
                let before = self.lines.len();
                let echo = &self.echo;
                self.lines.retain(|me| !echo.is_echo(me, &line));
                self.echo_removed += before - self.lines.len();
            }
        }

        let at = self.lines.partition_point(|l| l.start <= line.start);
        self.lines.insert(at, line);
        true
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for l in &self.lines {
            let _ = writeln!(out, "[{}] {}: {}", format_time(l.start), l.speaker.label(), l.text);
        }
        out
    }
}

pub fn format_time(seconds: f32) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(speaker: Speaker, start: f32, text: &str) -> Line {
        Line {
            speaker,
            start,
            end: start + 2.0,
            text: text.into(),
            language: None,
        }
    }

    #[test]
    fn clean_text_drops_hallucinations() {
        assert_eq!(clean_text(" [BLANK_AUDIO] "), None);
        assert_eq!(clean_text("(music)"), None);
        assert_eq!(clean_text("Thank you."), None);
        assert_eq!(clean_text("字幕由Amara.org社区提供"), None);
        assert_eq!(clean_text("♪ ♪"), None);
        assert_eq!(clean_text("..."), None);
        assert_eq!(clean_text("  We ship   on Friday. "), Some("We ship on Friday.".into()));
        assert_eq!(clean_text("我们周五发布。"), Some("我们周五发布。".into()));
    }

    #[test]
    fn containment_handles_english_and_chinese() {
        let sentence = "And so, my fellow Americans, ask not what your country can do for you";
        assert!(containment("Ask not!", sentence) > 0.99);
        assert!(containment("我们周五发布", "好的，我们周五发布新版本") > 0.99);
        assert!(containment("The budget is approved", "Let's meet on Monday") < 0.3);
        assert_eq!(containment("", "abc"), 0.0);
    }

    #[test]
    fn lines_are_kept_in_time_order() {
        let mut t = Transcript::default();
        t.add_final(0, line(Speaker::Others, 5.0, "second"));
        t.add_final(0, line(Speaker::Me, 1.0, "first"));
        t.add_final(1, line(Speaker::Others, 9.0, "third"));
        let texts: Vec<_> = t.lines().iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["first", "second", "third"]);
    }

    #[test]
    fn echo_of_others_on_the_mic_is_removed_either_order() {
        let mut t = Transcript::default();
        t.add_final(0, line(Speaker::Others, 10.0, "Can everyone see my screen now?"));
        assert!(!t.add_final(0, line(Speaker::Me, 10.3, "can everyone see my screen now")));
        assert_eq!(t.lines().len(), 1);

        // Me arrives first, Others later: the Me line is taken back out.
        t.add_final(1, line(Speaker::Me, 20.0, "Let's look at the numbers"));
        t.add_final(1, line(Speaker::Others, 19.8, "Let's look at the numbers."));
        assert_eq!(t.lines().len(), 2);
        assert!(t.lines().iter().all(|l| l.speaker == Speaker::Others));
        assert_eq!(t.echo_removed, 2);
    }

    #[test]
    fn echo_split_into_fragments_is_removed() {
        let mut t = Transcript::default();
        for (i, (start, text)) in [
            (0.0, "And so, my fellow Americans,"),
            (2.0, "Ask not!"),
            (5.0, "What your country can do for you."),
        ]
        .into_iter()
        .enumerate()
        {
            t.add_final(i as u64, line(Speaker::Me, start, text));
        }
        let mut others = line(
            Speaker::Others,
            0.0,
            "And so, my fellow Americans, ask not what your country can do for you.",
        );
        others.end = 8.0;
        t.add_final(0, others);
        assert_eq!(t.lines().len(), 1);
        assert_eq!(t.echo_removed, 3);
    }

    #[test]
    fn echo_is_caught_while_others_are_still_talking() {
        let mut t = Transcript::default();
        // Mic fragment first, then Others' live preview reveals it as echo.
        t.add_final(0, line(Speaker::Me, 0.0, "And so, my fellow Americans,"));
        t.set_preview(Speaker::Others, 0, 0.0, Some("And so my fellow Americans, ask not what".into()));
        assert!(t.is_empty());
        // A later fragment is dropped straight away against the preview.
        assert!(!t.add_final(1, line(Speaker::Me, 2.0, "Ask not!")));
        assert_eq!(t.echo_removed, 2);
    }

    #[test]
    fn short_replies_during_their_speech_are_kept() {
        let mut t = Transcript::default();
        t.add_final(0, line(Speaker::Others, 10.0, "Yes, exactly, and we ship on Friday."));
        assert!(t.add_final(0, line(Speaker::Me, 10.5, "Yes.")));
    }

    #[test]
    fn real_replies_and_distant_repeats_are_kept() {
        let mut t = Transcript::default();
        t.add_final(0, line(Speaker::Others, 10.0, "Can everyone see my screen?"));
        assert!(t.add_final(0, line(Speaker::Me, 12.0, "Yes, I can see it.")));
        // Same words a minute later are a real repeat, not echo.
        assert!(t.add_final(1, line(Speaker::Me, 70.0, "Can everyone see my screen?")));
    }

    #[test]
    fn echo_filter_can_be_turned_off() {
        let mut t = Transcript::default();
        t.echo.enabled = false;
        t.add_final(0, line(Speaker::Others, 1.0, "hello there everyone"));
        assert!(t.add_final(0, line(Speaker::Me, 1.0, "hello there everyone")));
    }

    #[test]
    fn previews_are_replaced_and_cleared_by_the_final() {
        let mut t = Transcript::default();
        t.set_preview(Speaker::Me, 0, 0.0, Some("hel".into()));
        t.set_preview(Speaker::Me, 0, 0.0, Some("hello wor".into()));
        assert_eq!(t.preview(Speaker::Me).unwrap().text, "hello wor");
        t.add_final(0, line(Speaker::Me, 0.0, "hello world"));
        assert!(t.preview(Speaker::Me).is_none());
        // A stale partial for the finished utterance does not come back.
        t.set_preview(Speaker::Me, 0, 0.0, Some("hello wor".into()));
        assert!(t.preview(Speaker::Me).is_none());
        t.set_preview(Speaker::Me, 1, 3.0, Some("next".into()));
        t.set_preview(Speaker::Me, 0, 0.0, Some("hello wor".into()));
        assert_eq!(t.preview(Speaker::Me).unwrap().text, "next");
    }

    #[test]
    fn text_export() {
        let mut t = Transcript::default();
        t.add_final(0, line(Speaker::Me, 3.0, "Hi."));
        t.add_final(0, line(Speaker::Others, 3725.0, "Bye."));
        assert_eq!(t.to_text(), "[00:03] Me: Hi.\n[1:02:05] Others: Bye.\n");
    }
}
