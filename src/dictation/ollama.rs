//! Optional clean-up of a dictation by a language model: punctuation and
//! mixed Chinese/English spacing, nothing else. The model is a local Ollama
//! one, or one behind an internet API in the OpenAI (chat completions) or
//! Claude (Messages) format. Any failure keeps the text as Whisper wrote it,
//! so a dictation is never lost to it.
//!
//! The JSON is written and read by hand: one request, one string back, not
//! worth a JSON library.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};

/// The kind of service the clean-up talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Api {
    /// Ollama's own `/api/generate`.
    #[default]
    Ollama,
    /// OpenAI's `/v1/chat/completions`, which many services and local
    /// servers (LM Studio, vLLM, …) also answer.
    OpenAi,
    /// The Claude API's `/v1/messages`.
    Anthropic,
}

impl Api {
    pub const ALL: [Api; 3] = [Api::Ollama, Api::OpenAi, Api::Anthropic];

    /// The name in settings.txt.
    pub fn id(self) -> &'static str {
        match self {
            Api::Ollama => "ollama",
            Api::OpenAi => "openai",
            Api::Anthropic => "anthropic",
        }
    }

    pub fn from_id(id: &str) -> Option<Api> {
        Api::ALL.into_iter().find(|a| a.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Api::Ollama => "Ollama (this computer)",
            Api::OpenAi => "OpenAI-compatible API",
            Api::Anthropic => "Claude API (Anthropic)",
        }
    }

    pub fn default_url(self) -> &'static str {
        match self {
            Api::Ollama => DEFAULT_URL,
            Api::OpenAi => "https://api.openai.com/v1/chat/completions",
            Api::Anthropic => "https://api.anthropic.com/v1/messages",
        }
    }

    /// Empty where the service decides which models there are.
    pub fn default_model(self) -> &'static str {
        match self {
            Api::Ollama => DEFAULT_MODEL,
            Api::OpenAi => "",
            Api::Anthropic => "claude-opus-5-5",
        }
    }

    /// Where the API key is read from when none is set in Settings.
    pub fn key_env(self) -> Option<&'static str> {
        match self {
            Api::Ollama => None,
            Api::OpenAi => Some("OPENAI_API_KEY"),
            Api::Anthropic => Some("ANTHROPIC_API_KEY"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OllamaConfig {
    pub api: Api,
    /// The endpoint, e.g. `http://localhost:11434/api/generate`.
    pub url: String,
    pub model: String,
    /// May be empty: Ollama and local servers need none.
    pub api_key: String,
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

/// The cleaned text, or `text` itself when the model fails or answers nothing.
pub fn cleanup(text: &str, cfg: &OllamaConfig) -> String {
    cleanup_with(text, cfg.api, &cfg.model, |body| post(cfg, body))
}

/// `send` posts a request body and returns the response body.
pub fn cleanup_with(text: &str, api: Api, model: &str, send: impl FnOnce(&str) -> Result<String>) -> String {
    let body = request_body(api, text, model);
    match send(&body).and_then(|reply| reply_text(api, &reply)) {
        Ok(cleaned) if !cleaned.trim().is_empty() => cleaned.trim().to_string(),
        Ok(_) => text.to_string(),
        Err(e) => {
            eprintln!("Clean-up failed, keeping the original text: {e:#}");
            text.to_string()
        }
    }
}

fn post(cfg: &OllamaConfig, body: &str) -> Result<String> {
    let mut config = ureq::Agent::config_builder().timeout_global(Some(cfg.timeout));
    // A model on this computer is reached directly: a proxy (from the
    // environment, or the Windows system proxy) usually cannot reach it.
    if is_local(&cfg.url) {
        config = config.proxy(None);
    }
    let agent: ureq::Agent = config.build().into();
    let mut request = agent.post(&cfg.url).header("Content-Type", "application/json");
    let key = cfg.api_key.trim();
    match cfg.api {
        Api::Ollama => {}
        Api::OpenAi if key.is_empty() => {}
        Api::OpenAi => request = request.header("Authorization", &format!("Bearer {key}")),
        Api::Anthropic => {
            request = request
                .header("x-api-key", key)
                .header("anthropic-version", "2026-10-21")
        }
    }
    let mut response = request
        .send(body)
        .with_context(|| format!("could not reach {}", cfg.url))?;
    if !response.status().is_success() {
        bail!("{} answered {}", cfg.url, response.status());
    }
    Ok(response.body_mut().read_to_string()?)
}

/// Whether `url` points at this computer.
pub fn is_local(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else {
        return false;
    };
    let host = uri.host().unwrap_or_default();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn request_body(api: Api, text: &str, model: &str) -> String {
    let model = json_string(model);
    let prompt = json_string(&prompt(text));
    match api {
        Api::Ollama => format!("{{\"model\":{model},\"prompt\":{prompt},\"stream\":false}}"),
        Api::OpenAi => format!("{{\"model\":{model},\"messages\":[{{\"role\":\"user\",\"content\":{prompt}}}]}}"),
        // Low effort: a short, simple job, and the dictation waits for it.
        // max_tokens is a ceiling, room for thinking as well as the text.
        Api::Anthropic => format!(
            "{{\"model\":{model},\"max_tokens\":16000,\"output_config\":{{\"effort\":\"low\"}},\
             \"messages\":[{{\"role\":\"user\",\"content\":{prompt}}}]}}"
        ),
    }
}

/// The model's text in a reply.
fn reply_text(api: Api, reply: &str) -> Result<String> {
    let json = Json::parse(reply).context("the reply is not JSON")?;
    let text = match api {
        Api::Ollama => json.get("response"),
        Api::OpenAi => json
            .get("choices")
            .and_then(|c| c.at(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content")),
        Api::Anthropic => {
            if json.get("stop_reason").and_then(Json::str) == Some("refusal") {
                bail!("the model declined");
            }
            // Thinking blocks may come first; the answer is the text block.
            match json.get("content") {
                Some(Json::Arr(blocks)) => blocks
                    .iter()
                    .find(|b| b.get("type").and_then(Json::str) == Some("text"))
                    .and_then(|b| b.get("text")),
                _ => None,
            }
        }
    };
    text.and_then(Json::str)
        .map(str::to_string)
        .context("no text in the reply")
}

/// A JSON string literal.
pub fn json_string(s: &str) -> String {
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

/// A JSON value, as far as the replies need: strings, arrays and objects;
/// numbers, true, false and null are not looked at.
#[derive(Debug, PartialEq)]
enum Json {
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
    Other,
}

impl Json {
    fn parse(s: &str) -> Option<Json> {
        Parser { s: s.as_bytes(), i: 0 }.value()
    }

    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn at(&self, index: usize) -> Option<&Json> {
        match self {
            Json::Arr(items) => items.get(index),
            _ => None,
        }
    }

    fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
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

    /// The items of an array or the fields of an object, after its opening
    /// bracket: `item` reads one, up to the `close` bracket.
    fn items(&mut self, close: u8, mut item: impl FnMut(&mut Self) -> Option<()>) -> Option<()> {
        self.i += 1;
        self.ws();
        if self.peek()? == close {
            self.i += 1;
            return Some(());
        }
        loop {
            item(self)?;
            self.ws();
            match self.next()? {
                b',' => continue,
                b if b == close => return Some(()),
                _ => return None,
            }
        }
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match self.peek()? {
            b'"' => self.string().map(Json::Str),
            b'[' => {
                let mut items = Vec::new();
                self.items(b']', |p| {
                    items.push(p.value()?);
                    Some(())
                })?;
                Some(Json::Arr(items))
            }
            b'{' => {
                let mut fields = Vec::new();
                self.items(b'}', |p| {
                    let key = p.string()?;
                    p.expect(b':')?;
                    fields.push((key, p.value()?));
                    Some(())
                })?;
                Some(Json::Obj(fields))
            }
            _ => {
                // A number, true, false or null.
                let start = self.i;
                while self
                    .peek()
                    .is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace())
                {
                    self.i += 1;
                }
                (self.i > start).then_some(Json::Other)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_urls_skip_the_proxy() {
        assert!(is_local(DEFAULT_URL));
        assert!(is_local("http://127.0.0.1:11434/api/generate"));
        assert!(is_local("http://[::1]:11434/api/generate"));
        assert!(!is_local("http://192.168.1.20:11434/api/generate"));
        assert!(!is_local("https://ollama.example.com/api/generate"));
    }

    fn field<'a>(json: &'a Json, key: &str) -> Option<&'a str> {
        json.get(key).and_then(Json::str)
    }

    #[test]
    fn request_is_valid_json_with_the_prompt() {
        let body = request_body(Api::Ollama, "He said \"hi\"\n", "qwen2.5:3b");
        assert!(body.starts_with("{\"model\":\"qwen2.5:3b\",\"prompt\":\"你是语音听写文本的清理器。"));
        assert!(body.ends_with("He said \\\"hi\\\"\\n\",\"stream\":false}"));
        let json = Json::parse(&body).unwrap();
        assert_eq!(field(&json, "model"), Some("qwen2.5:3b"));
        assert!(field(&json, "prompt").unwrap().ends_with("：\n\nHe said \"hi\"\n"));
        assert_eq!(json_string("a\u{1}b"), "\"a\\u0001b\"");
    }

    #[test]
    fn api_requests_carry_the_prompt_as_a_user_message() {
        for api in [Api::OpenAi, Api::Anthropic] {
            let json = Json::parse(&request_body(api, "hi", "m")).unwrap();
            assert_eq!(field(&json, "model"), Some("m"));
            let message = json.get("messages").and_then(|m| m.at(0)).unwrap();
            assert_eq!(field(message, "role"), Some("user"));
            assert!(field(message, "content").unwrap().ends_with("：\n\nhi"));
        }
        let json = Json::parse(&request_body(Api::Anthropic, "hi", "m")).unwrap();
        assert_eq!(json.get("max_tokens"), Some(&Json::Other));
    }

    #[test]
    fn reads_the_response_field_past_other_values() {
        let reply = r#"{"model":"q","created_at":"2026","context":[1,{"a":"}"},[2]],"response":"你好，world \ud83d\ude00\t\"x\"","done":true}"#;
        assert_eq!(
            reply_text(Api::Ollama, reply).ok().as_deref(),
            Some("你好，world 😀\t\"x\"")
        );
        assert!(reply_text(Api::Ollama, r#"{"done":true}"#).is_err());
        assert!(reply_text(Api::Ollama, "not json").is_err());
        assert!(reply_text(Api::Ollama, r#"{"response":"cut"#).is_err());
    }

    #[test]
    fn reads_openai_and_claude_replies() {
        let openai = r#"{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"你好，world."},"finish_reason":"stop"}],"usage":{"total_tokens":9}}"#;
        assert_eq!(reply_text(Api::OpenAi, openai).ok().as_deref(), Some("你好，world."));
        assert!(reply_text(Api::OpenAi, r#"{"choices":[]}"#).is_err());

        let claude = r#"{"id":"msg","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"","signature":"s"},{"type":"text","text":"你好，world."}],"stop_reason":"end_turn","usage":{"input_tokens":5}}"#;
        assert_eq!(reply_text(Api::Anthropic, claude).ok().as_deref(), Some("你好，world."));
        let refused = r#"{"content":[{"type":"text","text":"no"}],"stop_reason":"refusal"}"#;
        assert!(reply_text(Api::Anthropic, refused).is_err());
    }

    #[test]
    fn falls_back_to_the_raw_text() {
        let raw = "hello world";
        let ollama = |reply: &str| {
            let reply = reply.to_string();
            cleanup_with(raw, Api::Ollama, "m", move |_| Ok(reply))
        };
        assert_eq!(ollama(r#"{"response":" Hello, world. "}"#), "Hello, world.");
        assert_eq!(ollama(r#"{"response":"  "}"#), raw);
        assert_eq!(ollama("<html>502</html>"), raw);
        assert_eq!(
            cleanup_with(raw, Api::Anthropic, "m", |_| anyhow::bail!("connection refused")),
            raw
        );
    }

    #[test]
    fn api_ids_round_trip() {
        for api in Api::ALL {
            assert_eq!(Api::from_id(api.id()), Some(api));
        }
        assert_eq!(Api::from_id("nonsense"), None);
    }
}
