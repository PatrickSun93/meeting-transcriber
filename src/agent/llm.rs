//! A chat model, streamed: Ollama's `/api/chat` (one JSON object per line),
//! or an OpenAI-compatible `/v1/chat/completions` or the Claude Messages
//! API (both server-sent events). The same three kinds of service as the
//! dictation clean-up (`dictation::ollama`), with history and tools.
//!
//! The parsers take the stream one line at a time and are tested on
//! recorded streams; `stream` does the HTTP and stops reading as soon as
//! it is told to (barge-in), which closes the connection and so stops the
//! model.

use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

pub use crate::dictation::ollama::Api;
use crate::dictation::ollama::is_local;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// The service's id for the call (OpenAI, Claude); made up for Ollama.
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// The assistant's tool calls.
    pub tool_calls: Vec<ToolCall>,
    /// A tool result: the call it answers.
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
}

impl Message {
    fn new(role: Role, content: &str) -> Self {
        Self {
            role,
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn system(content: &str) -> Self {
        Self::new(Role::System, content)
    }

    pub fn user(content: &str) -> Self {
        Self::new(Role::User, content)
    }

    pub fn assistant(content: &str) -> Self {
        Self::new(Role::Assistant, content)
    }

    pub fn assistant_calls(content: &str, calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls: calls,
            ..Self::new(Role::Assistant, content)
        }
    }

    pub fn tool_result(call: &ToolCall, content: &str) -> Self {
        Self {
            tool_call_id: Some(call.id.clone()),
            tool_name: Some(call.name.clone()),
            ..Self::new(Role::Tool, content)
        }
    }
}

/// A tool the model may call, with a JSON Schema for its arguments.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmConfig {
    pub api: Api,
    pub url: String,
    pub model: String,
    pub api_key: String,
}

pub const OLLAMA_CHAT_URL: &str = "http://localhost:11434/api/chat";
pub const DEFAULT_OLLAMA_MODEL: &str = "gemma4:12b";

impl LlmConfig {
    pub fn default_url(api: Api) -> &'static str {
        match api {
            Api::Ollama => OLLAMA_CHAT_URL,
            other => other.default_url(),
        }
    }

    pub fn default_model(api: Api) -> &'static str {
        match api {
            Api::Ollama => DEFAULT_OLLAMA_MODEL,
            other => other.default_model(),
        }
    }
}

/// What the stream brought.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Text(String),
    ToolCall(ToolCall),
    Done,
}

/// The request body for `messages`, streamed.
pub fn request_body(api: Api, model: &str, messages: &[Message], tools: &[ToolSpec]) -> String {
    let body = match api {
        Api::Ollama => {
            let mut body = json!({
                "model": model,
                "messages": messages.iter().map(ollama_message).collect::<Vec<_>>(),
                "stream": true,
                // Thinking first would cost seconds before the first word.
                "think": false,
                // Stays loaded between turns (and calls).
                "keep_alive": "30m",
                "options": {"temperature": 0.4},
            });
            if !tools.is_empty() {
                body["tools"] = tools.iter().map(openai_tool).collect();
            }
            body
        }
        Api::OpenAi => {
            let mut body = json!({
                "model": model,
                "messages": messages.iter().map(openai_message).collect::<Vec<_>>(),
                "stream": true,
            });
            if !tools.is_empty() {
                body["tools"] = tools.iter().map(openai_tool).collect();
            }
            body
        }
        Api::Anthropic => {
            let system: Vec<&str> = messages
                .iter()
                .filter(|m| m.role == Role::System)
                .map(|m| m.content.as_str())
                .collect();
            let mut body = json!({
                "model": model,
                "max_tokens": 1024,
                // A phone answer: little to think about, and someone waits.
                "output_config": {"effort": "low"},
                "messages": anthropic_messages(messages),
                "stream": true,
            });
            if !system.is_empty() {
                body["system"] = Value::String(system.join("\n\n"));
            }
            if !tools.is_empty() {
                body["tools"] = tools
                    .iter()
                    .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
                    .collect();
            }
            body
        }
    };
    body.to_string()
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn openai_tool(t: &ToolSpec) -> Value {
    json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}})
}

fn ollama_message(m: &Message) -> Value {
    let mut v = json!({"role": role_name(m.role), "content": m.content});
    if !m.tool_calls.is_empty() {
        v["tool_calls"] = m
            .tool_calls
            .iter()
            .map(|c| json!({"function": {"name": c.name, "arguments": c.arguments}}))
            .collect();
    }
    if let Some(name) = &m.tool_name {
        v["tool_name"] = Value::String(name.clone());
    }
    v
}

fn openai_message(m: &Message) -> Value {
    let mut v = json!({"role": role_name(m.role), "content": m.content});
    if !m.tool_calls.is_empty() {
        v["tool_calls"] = m
            .tool_calls
            .iter()
            .map(|c| {
                json!({"id": c.id, "type": "function",
                       "function": {"name": c.name, "arguments": c.arguments.to_string()}})
            })
            .collect();
    }
    if let Some(id) = &m.tool_call_id {
        v["tool_call_id"] = Value::String(id.clone());
    }
    v
}

/// Claude's messages: the system prompt goes apart, tool results are user
/// content blocks, and one role's messages in a row become one message.
fn anthropic_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<(&str, Vec<Value>)> = Vec::new();
    for m in messages {
        let (role, blocks) = match m.role {
            Role::System => continue,
            Role::User => ("user", vec![json!({"type": "text", "text": m.content})]),
            Role::Tool => (
                "user",
                vec![json!({"type": "tool_result", "tool_use_id": m.tool_call_id, "content": m.content})],
            ),
            Role::Assistant => {
                let mut blocks = Vec::new();
                if !m.content.trim().is_empty() {
                    blocks.push(json!({"type": "text", "text": m.content}));
                }
                for c in &m.tool_calls {
                    blocks.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.arguments}));
                }
                ("assistant", blocks)
            }
        };
        if blocks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((last, existing)) if *last == role => existing.extend(blocks),
            _ => out.push((role, blocks)),
        }
    }
    out.into_iter()
        .map(|(role, content)| json!({"role": role, "content": content}))
        .collect()
}

/// Reads a stream one line at a time.
#[derive(Debug)]
pub struct StreamParser {
    api: Api,
    calls: usize,
    /// Tool calls being streamed in pieces: OpenAI by index, Claude by
    /// content block. (id, name, arguments so far)
    pending: Vec<(usize, String, String, String)>,
}

impl StreamParser {
    pub fn new(api: Api) -> Self {
        Self {
            api,
            calls: 0,
            pending: Vec::new(),
        }
    }

    /// The events in one line of the response.
    pub fn line(&mut self, line: &str) -> Result<Vec<StreamEvent>> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(Vec::new());
        }
        match self.api {
            Api::Ollama => self.ollama(line),
            // Server-sent events: only `data:` lines carry anything.
            Api::OpenAi | Api::Anthropic => {
                let Some(data) = line.strip_prefix("data:") else {
                    return Ok(Vec::new());
                };
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(self.finish_calls(true));
                }
                let v: Value = serde_json::from_str(data).context("a streamed event is not JSON")?;
                if self.api == Api::OpenAi {
                    self.openai(&v)
                } else {
                    self.anthropic(&v)
                }
            }
        }
    }

    fn next_id(&mut self) -> String {
        self.calls += 1;
        format!("call_{}", self.calls)
    }

    fn ollama(&mut self, line: &str) -> Result<Vec<StreamEvent>> {
        let v: Value = serde_json::from_str(line).context("a streamed line is not JSON")?;
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            bail!("the model answered: {err}");
        }
        let mut out = Vec::new();
        if let Some(message) = v.get("message") {
            if let Some(text) = message.get("content").and_then(Value::as_str)
                && !text.is_empty()
            {
                out.push(StreamEvent::Text(text.to_string()));
            }
            for call in message
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let f = call.get("function").unwrap_or(call);
                let name = f.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let arguments = match f.get("arguments") {
                    // Some models send the arguments as a JSON string.
                    Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
                    Some(v) => v.clone(),
                    None => json!({}),
                };
                let id = match call.get("id").and_then(Value::as_str) {
                    Some(id) => id.to_string(),
                    None => self.next_id(),
                };
                out.push(StreamEvent::ToolCall(ToolCall { id, name, arguments }));
            }
        }
        if v.get("done").and_then(Value::as_bool) == Some(true) {
            out.push(StreamEvent::Done);
        }
        Ok(out)
    }

    fn openai(&mut self, v: &Value) -> Result<Vec<StreamEvent>> {
        if let Some(err) = v.get("error") {
            bail!(
                "the model answered: {}",
                err.get("message").and_then(Value::as_str).unwrap_or("an error")
            );
        }
        let mut out = Vec::new();
        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
            return Ok(out);
        };
        let delta = choice.get("delta").unwrap_or(&Value::Null);
        if let Some(text) = delta.get("content").and_then(Value::as_str)
            && !text.is_empty()
        {
            out.push(StreamEvent::Text(text.to_string()));
        }
        for call in delta.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
            let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let f = call.get("function").unwrap_or(&Value::Null);
            let piece = f.get("arguments").and_then(Value::as_str).unwrap_or_default();
            match self.pending.iter_mut().find(|p| p.0 == index) {
                Some(p) => p.3.push_str(piece),
                None => {
                    let id = match call.get("id").and_then(Value::as_str) {
                        Some(id) => id.to_string(),
                        None => self.next_id(),
                    };
                    let name = f.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                    self.pending.push((index, id, name, piece.to_string()));
                }
            }
        }
        if choice.get("finish_reason").and_then(Value::as_str).is_some() {
            out.extend(self.finish_calls(false));
        }
        Ok(out)
    }

    fn anthropic(&mut self, v: &Value) -> Result<Vec<StreamEvent>> {
        let mut out = Vec::new();
        let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        match v.get("type").and_then(Value::as_str).unwrap_or_default() {
            "error" => bail!(
                "the model answered: {}",
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("an error")
            ),
            "content_block_start" => {
                let block = v.get("content_block").unwrap_or(&Value::Null);
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    self.pending.push((index, id, name, String::new()));
                }
            }
            "content_block_delta" => {
                let delta = v.get("delta").unwrap_or(&Value::Null);
                match delta.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            out.push(StreamEvent::Text(text.to_string()));
                        }
                    }
                    "input_json_delta" => {
                        let piece = delta.get("partial_json").and_then(Value::as_str).unwrap_or_default();
                        if let Some(p) = self.pending.iter_mut().find(|p| p.0 == index) {
                            p.3.push_str(piece);
                        }
                    }
                    // Thinking is not spoken.
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(at) = self.pending.iter().position(|p| p.0 == index) {
                    let (_, id, name, args) = self.pending.remove(at);
                    out.push(StreamEvent::ToolCall(ToolCall {
                        id,
                        name,
                        arguments: parse_arguments(&args),
                    }));
                }
            }
            "message_delta" => {
                if v.pointer("/delta/stop_reason").and_then(Value::as_str) == Some("refusal") {
                    bail!("the model declined");
                }
            }
            "message_stop" => out.push(StreamEvent::Done),
            _ => {}
        }
        Ok(out)
    }

    /// The tool calls streamed so far, complete.
    fn finish_calls(&mut self, done: bool) -> Vec<StreamEvent> {
        let mut out: Vec<StreamEvent> = std::mem::take(&mut self.pending)
            .into_iter()
            .map(|(_, id, name, args)| {
                StreamEvent::ToolCall(ToolCall {
                    id,
                    name,
                    arguments: parse_arguments(&args),
                })
            })
            .collect();
        if done {
            out.push(StreamEvent::Done);
        }
        out
    }
}

/// Streamed arguments: a JSON object, or the raw text when it is not one
/// (the tool then says the arguments were wrong).
fn parse_arguments(s: &str) -> Value {
    if s.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
}

/// Sends `messages` and calls `on_event` for everything streamed back,
/// until the stream ends or `cancel` is set. Returns false if cancelled.
pub fn stream(
    cfg: &LlmConfig,
    messages: &[Message],
    tools: &[ToolSpec],
    cancel: &Arc<AtomicBool>,
    mut on_event: impl FnMut(StreamEvent),
) -> Result<bool> {
    let mut config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        // Loading a model into memory can take a while the first time.
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .timeout_per_read(Some(Duration::from_secs(60)));
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
    let body = request_body(cfg.api, &cfg.model, messages, tools);
    let response = match request.send(&body) {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(code)) => bail!("{} answered {code}", cfg.url),
        Err(e) => return Err(e).with_context(|| format!("could not reach {}", cfg.url)),
    };
    let mut parser = StreamParser::new(cfg.api);
    let reader = BufReader::new(response.into_body().into_reader());
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) {
            // Dropping the reader closes the connection: the model stops.
            return Ok(false);
        }
        let line = line.context("the stream broke off")?;
        for event in parser.line(&line)? {
            let done = event == StreamEvent::Done;
            on_event(event);
            if done {
                return Ok(true);
            }
        }
    }
    on_event(StreamEvent::Done);
    Ok(!cancel.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(api: Api, stream: &str) -> Vec<StreamEvent> {
        let mut p = StreamParser::new(api);
        stream.lines().flat_map(|l| p.line(l).unwrap()).collect()
    }

    fn text_of(events: &[StreamEvent]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ollama_ndjson_text_and_done() {
        let stream = r#"{"model":"gemma4:12b","created_at":"2026-11-13T04:55:37Z","message":{"role":"assistant","content":"We"},"done":false}
{"model":"gemma4:12b","message":{"role":"assistant","content":" have 9:00 \"open\"."},"done":false}

{"model":"gemma4:12b","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","eval_count":16}"#;
        let events = parse_all(Api::Ollama, stream);
        assert_eq!(text_of(&events), "We have 9:00 \"open\".");
        assert_eq!(events.last(), Some(&StreamEvent::Done));
    }

    #[test]
    fn ollama_tool_calls_with_object_or_string_arguments() {
        // Recorded from gemma4:12b.
        let stream = r#"{"model":"gemma4:12b","message":{"role":"assistant","content":"","tool_calls":[{"id":"call_dms95ved","function":{"index":0,"name":"check_availability","arguments":{"day":"tomorrow"}}}]},"done":false}
{"model":"x","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"book_appointment","arguments":"{\"time\":\"09:00\"}"}}]},"done":false}
{"model":"gemma4:12b","message":{"role":"assistant","content":""},"done":true}"#;
        let events = parse_all(Api::Ollama, stream);
        assert_eq!(
            events,
            [
                StreamEvent::ToolCall(ToolCall {
                    id: "call_dms95ved".into(),
                    name: "check_availability".into(),
                    arguments: json!({"day": "tomorrow"}),
                }),
                StreamEvent::ToolCall(ToolCall {
                    id: "call_1".into(),
                    name: "book_appointment".into(),
                    arguments: json!({"time": "09:00"}),
                }),
                StreamEvent::Done,
            ]
        );
    }

    #[test]
    fn ollama_errors_and_broken_lines() {
        let mut p = StreamParser::new(Api::Ollama);
        assert!(p.line(r#"{"error":"model 'x' not found"}"#).is_err());
        assert!(p.line(r#"{"message":{"content":"cut"#).is_err());
    }

    #[test]
    fn openai_sse_text_and_tool_call_in_pieces() {
        let stream = r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}

data: {"choices":[{"index":0,"delta":{"content":"Let me "}}]}
data: {"choices":[{"index":0,"delta":{"content":"check."}}]}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"check_availability","arguments":""}}]}}]}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"da"}}]}}]}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"y\":\"monday\"}"}}]}}]}
data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}
data: [DONE]"#;
        let events = parse_all(Api::OpenAi, stream);
        assert_eq!(text_of(&events), "Let me check.");
        assert!(events.contains(&StreamEvent::ToolCall(ToolCall {
            id: "call_a".into(),
            name: "check_availability".into(),
            arguments: json!({"day": "monday"}),
        })));
        assert_eq!(events.last(), Some(&StreamEvent::Done));
    }

    #[test]
    fn claude_sse_text_tool_use_and_thinking() {
        let stream = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}
data: {"type":"content_block_stop","index":0}
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"One moment."}}
data: {"type":"content_block_stop","index":1}
data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"book_appointment","input":{}}}
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"time\": \"10"}}
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":":30\"}"}}
data: {"type":"content_block_stop","index":2}
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}
data: {"type":"message_stop"}"#;
        let events = parse_all(Api::Anthropic, stream);
        assert_eq!(
            events,
            [
                StreamEvent::Text("One moment.".into()),
                StreamEvent::ToolCall(ToolCall {
                    id: "toolu_1".into(),
                    name: "book_appointment".into(),
                    arguments: json!({"time": "10:30"}),
                }),
                StreamEvent::Done,
            ]
        );
        let mut p = StreamParser::new(Api::Anthropic);
        assert!(
            p.line(r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)
                .is_err()
        );
    }

    fn conversation() -> Vec<Message> {
        let call = ToolCall {
            id: "c1".into(),
            name: "check_availability".into(),
            arguments: json!({"day": "monday"}),
        };
        vec![
            Message::system("Be brief."),
            Message::user("Anything Monday?"),
            Message::assistant_calls("", vec![call.clone()]),
            Message::tool_result(&call, r#"{"open":["09:00"]}"#),
            Message::assistant("Nine o'clock."),
        ]
    }

    #[test]
    fn requests_carry_history_and_tools_in_each_format() {
        let tools = [ToolSpec {
            name: "check_availability",
            description: "Open slots",
            parameters: json!({"type": "object"}),
        }];
        let msgs = conversation();

        let ollama: Value = serde_json::from_str(&request_body(Api::Ollama, "gemma4:12b", &msgs, &tools)).unwrap();
        assert_eq!(ollama["stream"], json!(true));
        assert_eq!(
            ollama["messages"][2]["tool_calls"][0]["function"]["arguments"]["day"],
            "monday"
        );
        assert_eq!(ollama["messages"][3]["role"], "tool");
        assert_eq!(ollama["messages"][3]["tool_name"], "check_availability");
        assert_eq!(ollama["tools"][0]["function"]["name"], "check_availability");

        let openai: Value = serde_json::from_str(&request_body(Api::OpenAi, "m", &msgs, &tools)).unwrap();
        assert_eq!(
            openai["messages"][2]["tool_calls"][0]["function"]["arguments"],
            r#"{"day":"monday"}"#
        );
        assert_eq!(openai["messages"][3]["tool_call_id"], "c1");

        let claude: Value = serde_json::from_str(&request_body(Api::Anthropic, "m", &msgs, &tools)).unwrap();
        assert_eq!(claude["system"], "Be brief.");
        let m = claude["messages"].as_array().unwrap();
        let roles: Vec<&str> = m.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
        assert_eq!(m[1]["content"][0]["type"], "tool_use");
        assert_eq!(m[2]["content"][0]["tool_use_id"], "c1");
        assert_eq!(claude["tools"][0]["input_schema"]["type"], "object");

        // No tools: no "tools" key at all.
        let plain: Value = serde_json::from_str(&request_body(Api::Ollama, "m", &msgs, &[])).unwrap();
        assert!(plain.get("tools").is_none());
    }
}
