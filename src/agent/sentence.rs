//! Cuts the model's streamed text into sentences, so the first one can be
//! spoken while the rest is still being written.
//!
//! A sentence ends at `.`, `!` or `?` followed by a space (so "10.30" and
//! "e.g." in the middle of a word don't end one), at a Chinese or Japanese
//! full stop, question or exclamation mark, or at a line break. A few
//! abbreviations ("Dr.", "Mr.") don't end one either. A very long sentence
//! is cut at a comma or a space so the voice does not wait for it.

/// Words that end with a full stop without ending the sentence.
const ABBREVIATIONS: &[&str] = &[
    "dr", "mr", "mrs", "ms", "st", "vs", "etc", "e.g", "i.e", "a.m", "p.m", "no", "approx", "jr", "sr",
];

/// A sentence longer than this is cut at its last comma or space.
const MAX_CHARS: usize = 220;

#[derive(Debug, Default)]
pub struct SentenceSplitter {
    buf: String,
}

impl SentenceSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds streamed text; returns the sentences it completed.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        while let Some(end) = self.boundary() {
            let rest = self.buf.split_off(end);
            let sentence = std::mem::replace(&mut self.buf, rest);
            if let Some(s) = tidy(&sentence) {
                out.push(s);
            }
        }
        out
    }

    /// The text left when the stream ends.
    pub fn finish(&mut self) -> Option<String> {
        tidy(&std::mem::take(&mut self.buf))
    }

    /// The byte index just after the first sentence in `buf`, if one is complete.
    fn boundary(&self) -> Option<usize> {
        let chars: Vec<(usize, char)> = self.buf.char_indices().collect();
        for (n, &(i, c)) in chars.iter().enumerate() {
            let after = i + c.len_utf8();
            match c {
                '\n' => return Some(after),
                '。' | '！' | '？' | '；' | '…' => return Some(after),
                '.' | '!' | '?' => {
                    // Decided by what follows: no next character yet, wait.
                    let next = chars.get(n + 1)?.1;
                    // "?!", "..." and closing quotes stay with the sentence.
                    if matches!(next, '.' | '!' | '?' | '"' | '\'' | ')' | '”' | '’') {
                        continue;
                    }
                    if next.is_whitespace() && !(c == '.' && is_abbreviation(&self.buf[..i])) {
                        return Some(after);
                    }
                }
                '"' | '\'' | ')' | '”' | '’' => {
                    // `He said "hi." Then…`: the quote closes the sentence.
                    let prev = n.checked_sub(1).map(|p| chars[p].1);
                    let next = chars.get(n + 1).map(|x| x.1);
                    if matches!(prev, Some('.' | '!' | '?')) && next.is_some_and(char::is_whitespace) {
                        return Some(after);
                    }
                    if matches!(prev, Some('.' | '!' | '?')) && next.is_none() {
                        return None;
                    }
                }
                _ => {}
            }
        }
        if self.buf.chars().count() > MAX_CHARS {
            let cut = self
                .buf
                .rfind([',', '，', '、'])
                .map(|i| i + self.buf[i..].chars().next().map_or(1, char::len_utf8))
                .or_else(|| self.buf.rfind(char::is_whitespace))?;
            return (cut > 0).then_some(cut);
        }
        None
    }
}

/// Whether the text before a full stop ends with an abbreviation.
fn is_abbreviation(before: &str) -> bool {
    let word = before.rsplit(char::is_whitespace).next().unwrap_or("");
    let word = word.trim_start_matches(['(', '"', '\'']).to_lowercase();
    // A single letter, as in "J. Smith" or "a.m." being written.
    (word.chars().count() == 1 && word.chars().all(char::is_alphabetic)) || ABBREVIATIONS.contains(&word.as_str())
}

/// The sentence as it will be spoken, or None if nothing in it can be said.
/// Markdown the model wrote despite being told not to (`**bold**`, `# `,
/// list dashes) would be read out, so it goes.
fn tidy(s: &str) -> Option<String> {
    let s: String = s.chars().filter(|c| !matches!(c, '*' | '#' | '`')).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_start_matches(['-', '•', ' ']).to_string();
    s.chars().any(char::is_alphanumeric).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(tokens: &[&str]) -> Vec<String> {
        let mut s = SentenceSplitter::new();
        let mut out: Vec<String> = tokens.iter().flat_map(|t| s.push(t)).collect();
        out.extend(s.finish());
        out
    }

    #[test]
    fn splits_at_the_end_of_each_sentence_as_tokens_arrive() {
        let mut s = SentenceSplitter::new();
        assert!(s.push("We have").is_empty());
        assert!(
            s.push(" openings tomorrow.").is_empty(),
            "waits for what follows the stop"
        );
        assert_eq!(s.push(" Would"), ["We have openings tomorrow."]);
        assert!(s.push(" you like one?").is_empty());
        assert_eq!(s.finish().as_deref(), Some("Would you like one?"));
    }

    #[test]
    fn times_decimals_and_abbreviations_do_not_end_a_sentence() {
        assert_eq!(
            split(&["Dr. Lee is free at 10.30 or 9:00 a.m. ", "tomorrow. Okay?"]),
            ["Dr. Lee is free at 10.30 or 9:00 a.m. tomorrow.", "Okay?"]
        );
        assert_eq!(split(&["Mrs. J. Smith is booked."]), ["Mrs. J. Smith is booked."]);
    }

    #[test]
    fn punctuation_runs_and_quotes_stay_with_their_sentence() {
        assert_eq!(split(&["Really?! Yes... ", "fine."]), ["Really?!", "Yes...", "fine."]);
        assert_eq!(
            split(&["She said \"see you.\" Then left."]),
            ["She said \"see you.\"", "Then left."]
        );
    }

    #[test]
    fn chinese_and_line_breaks() {
        assert_eq!(
            split(&["明天上午", "九点有空。您要预约吗？", "好的"]),
            ["明天上午九点有空。", "您要预约吗？", "好的"]
        );
        assert_eq!(split(&["First line\nSecond line"]), ["First line", "Second line"]);
    }

    #[test]
    fn nothing_sayable_is_dropped() {
        assert_eq!(split(&["... ", "\n\n", "  "]), Vec::<String>::new());
        assert_eq!(split(&["Sure. ", "- ", "Bye."]), ["Sure.", "Bye."]);
    }

    #[test]
    fn a_very_long_sentence_is_cut_at_a_comma() {
        let long = format!("{}, and then {}", "word ".repeat(40).trim(), "more ".repeat(10));
        let out = split(&[&long]);
        assert!(out.len() >= 2, "{out:?}");
        assert!(out[0].ends_with(','), "{:?}", out[0]);
        assert!(out.iter().all(|s| s.chars().count() <= MAX_CHARS));
    }
}
