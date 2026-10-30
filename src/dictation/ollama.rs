//! Optional clean-up of a dictation by a local Ollama model: punctuation and
//! mixed Chinese/English spacing, nothing else. Any failure keeps the text
//! as Whisper wrote it, so a dictation is never lost to it.
//!
//! The JSON is written and read by hand: one request, one string field back,
//! not worth a JSON library.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};

#[derive(Debug, Clone, PartialEq)]
pub struct OllamaConfig {
    /// The generate endpoint, e.g. `http://localhost:11434/api/generate`.
    pub url: String,
    pub model: String,
    pub timeout: Duration,
}

pub const DEFAULT_URL: &str = "http://localhost:11434/api/generate";
pub const DEFAULT_MODEL: &str = "qwen2.5:3b";

/// voice2cursor's prompt, word for word.
fn prompt(text: &str) -> String {
    format!(
        "你是语音听写文本的清理器。修正下面文本的标点符号和中英文混排格式\
         （中英文之间加空格、英文专有名词大小写），不改变内容和语序，\
         直接输出修正后的文本，不要任何解释：\n\n{text}"
    )
}

/// The cleaned text, or `text` itself when Ollama fails or answers nothing.
pub fn cleanup(text: &str, cfg: &OllamaConfig) -> String {
    cleanup_with(text, &cfg.model, |body| post(cfg, body))
}

/// `send` posts a request body and returns the response body.
pub fn cleanup_with(text: &str, model: &str, send: impl FnOnce(&str) -> Result<String>) -> String {
    let body = request_body(text, model);
    match send(&body).and_then(|reply| json_string_field(&reply, "response").context("no \"response\" in the reply")) {
        Ok(cleaned) if !cleaned.trim().is_empty() => cleaned.trim().to_string(),
        Ok(_) => text.to_string(),
        Err(e) => {
            eprintln!("Ollama clean-up failed, keeping the original text: {e:#}");
            text.to_string()
        }
    }
}

fn post(cfg: &OllamaConfig, body: &str) -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(cfg.timeout))
        .build()
        .into();
    let mut response = agent
        .post(&cfg.url)
        .header("Content-Type", "application/json")
        .send(body)
        .with_context(|| format!("could not reach Ollama at {}", cfg.url))?;
    if !response.status().is_success() {
        bail!("Ollama answered {}", response.status());
    }
    Ok(response.body_mut().read_to_string()?)
}

/// `{"model":…,"prompt":…,"stream":false}`.
fn request_body(text: &str, model: &str) -> String {
    format!(
        "{{\"model\":{},\"prompt\":{},\"stream\":false}}",
        json_string(model),
        json_string(&prompt(text))
    )
}

/// A JSON string literal.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The string value of `key` in a JSON object, skipping over every other
/// value (nested objects, arrays, numbers) without interpreting it.
fn json_string_field(json: &str, key: &str) -> Option<String> {
    let mut p = Parser {
        s: json.as_bytes(),
        i: 0,
    };
    p.expect(b'{')?;
    loop {
        p.ws();
        if p.peek()? == b'}' {
            return None;
        }
        let name = p.string()?;
        p.ws();
        p.expect(b':')?;
        p.ws();
        if name == key && p.peek()? == b'"' {
            return p.string();
        }
        p.skip_value()?;
        p.ws();
        match p.next()? {
            b',' => continue,
            _ => return None,
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.i += 1;
        Some(b)
    }

    fn ws(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Option<()> {
        self.ws();
        (self.next()? == b).then_some(())
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(digits, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.next()? {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let c = match self.next()? {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                // A surrogate pair: `\ud83d\ude00`.
                                if self.next()? != b'\\' || self.next()? != b'u' {
                                    return None;
                                }
                                let lo = self.hex4()?;
                                0x10000 + ((hi - 0xD800) << 10) + (lo.checked_sub(0xDC00)?)
                            } else {
                                hi
                            };
                            char::from_u32(code)?
                        }
                        other => other as char,
                    };
                    let mut buf = [0; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                b => out.push(b),
            }
        }
    }

    fn skip_value(&mut self) -> Option<()> {
        match self.peek()? {
            b'"' => self.string().map(drop),
            open @ (b'{' | b'[') => {
                let close = if open == b'{' { b'}' } else { b']' };
                self.i += 1;
                loop {
                    self.ws();
                    match self.peek()? {
                        b if b == close => {
                            self.i += 1;
                            return Some(());
                        }
                        b',' | b':' => self.i += 1,
                        _ => self.skip_value()?,
                    }
                }
            }
            _ => {
                // A number, true, false or null.
                while self
                    .peek()
                    .is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace())
                {
                    self.i += 1;
                }
                Some(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_is_valid_json_with_the_prompt() {
        let body = request_body("He said \"hi\"\n", "qwen2.5:3b");
        assert!(body.starts_with("{\"model\":\"qwen2.5:3b\",\"prompt\":\"你是语音听写文本的清理器。"));
        assert!(body.ends_with("He said \\\"hi\\\"\\n\",\"stream\":false}"));
        assert_eq!(json_string_field(&body, "model").as_deref(), Some("qwen2.5:3b"));
        assert!(
            json_string_field(&body, "prompt")
                .unwrap()
                .ends_with("：\n\nHe said \"hi\"\n")
        );
        assert_eq!(json_string("a\u{1}b"), "\"a\\u0001b\"");
    }

    #[test]
    fn reads_the_response_field_past_other_values() {
        let reply = r#"{"model":"q","created_at":"2026","context":[1,{"a":"}"},[2]],"response":"你好，world \ud83d\ude00\t\"x\"","done":true}"#;
        assert_eq!(
            json_string_field(reply, "response").as_deref(),
            Some("你好，world 😀\t\"x\"")
        );
        assert_eq!(json_string_field(r#"{"done":true}"#, "response"), None);
        assert_eq!(json_string_field("not json", "response"), None);
        assert_eq!(json_string_field(r#"{"response":"cut"#, "response"), None);
    }

    #[test]
    fn falls_back_to_the_raw_text() {
        let raw = "hello world";
        assert_eq!(
            cleanup_with(raw, "m", |_| Ok(r#"{"response":" Hello, world. "}"#.into())),
            "Hello, world."
        );
        assert_eq!(cleanup_with(raw, "m", |_| anyhow::bail!("connection refused")), raw);
        assert_eq!(cleanup_with(raw, "m", |_| Ok(r#"{"response":"  "}"#.into())), raw);
        assert_eq!(cleanup_with(raw, "m", |_| Ok("<html>502</html>".into())), raw);
    }
}
