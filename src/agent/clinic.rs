//! The demo scenario: the front desk of a made-up clinic taking calls, with
//! two tools, `check_availability` and `book_appointment`.
//!
//! DEMO DATA: the clinic, its opening hours and which times are taken are
//! invented and kept in memory; nothing is booked anywhere.
//!
//! Tools are offered to the model the native way (Ollama, OpenAI and Claude
//! all take a list of tools), or, with `--tools json`, described in the
//! prompt and answered as a JSON object alone in the reply, for models whose
//! own tool calling is unreliable. Either way the arguments are checked
//! here, and a wrong call gets an error back that the model can act on.

use chrono::{Datelike as _, Duration, NaiveDate, NaiveTime, Weekday};
use serde_json::{Value, json};

use super::llm::{ToolCall, ToolSpec};

pub const CLINIC: &str = "Maple Street Clinic";
pub const GREETING: &str = "Hi, thanks for calling Maple Street Clinic. How can I help you today?";

/// Weekdays shown to the model and bookable.
const DAYS_AHEAD: usize = 7;
/// Every half hour from 9 to 5, closed for lunch from 12 to 1.
const SLOTS: [(u32, u32); 14] = [
    (9, 0),
    (9, 30),
    (10, 0),
    (10, 30),
    (11, 0),
    (11, 30),
    (13, 0),
    (13, 30),
    (14, 0),
    (14, 30),
    (15, 0),
    (15, 30),
    (16, 0),
    (16, 30),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    /// The service's own tool calling.
    Native,
    /// A JSON object in the reply, described in the prompt.
    Json,
}

#[derive(Debug, Clone, PartialEq)]
struct Booking {
    date: NaiveDate,
    time: NaiveTime,
    name: String,
    reason: String,
    code: String,
}

#[derive(Debug)]
pub struct Clinic {
    today: NaiveDate,
    days: Vec<NaiveDate>,
    /// Taken by made-up patients.
    taken: Vec<(NaiveDate, NaiveTime)>,
    bookings: Vec<Booking>,
}

fn slot(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap_or_default()
}

fn is_open_day(d: NaiveDate) -> bool {
    !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
}

impl Clinic {
    /// The next weekdays from `today`, about a third of their times taken,
    /// always the same ones for the same day (so a demo can be rehearsed).
    pub fn demo(today: NaiveDate) -> Self {
        let days: Vec<NaiveDate> = (0..14)
            .map(|i| today + Duration::days(i))
            .filter(|d| is_open_day(*d))
            .take(DAYS_AHEAD)
            .collect();
        let mut taken = Vec::new();
        for d in &days {
            let seed = d.ordinal() as usize;
            for (i, (h, m)) in SLOTS.iter().enumerate() {
                if (seed + i * 7).is_multiple_of(3) {
                    taken.push((*d, slot(*h, *m)));
                }
            }
        }
        Self {
            today,
            days,
            taken,
            bookings: Vec::new(),
        }
    }

    pub fn tools() -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "check_availability",
                description: "List the open appointment times on one day. Call this before offering any time.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "date": {"type": "string", "description": "The day, as YYYY-MM-DD"},
                        "part_of_day": {"type": "string", "enum": ["morning", "afternoon", "any"],
                                        "description": "Only morning (before noon) or afternoon times"}
                    },
                    "required": ["date"]
                }),
            },
            ToolSpec {
                name: "book_appointment",
                description: "Book an open time for the caller. Only call this once the caller has \
                              chosen a time and told you their name.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "date": {"type": "string", "description": "YYYY-MM-DD"},
                        "time": {"type": "string", "description": "24-hour HH:MM, e.g. 09:30 or 14:00"},
                        "patient_name": {"type": "string"},
                        "reason": {"type": "string", "description": "Why they are coming in, briefly"}
                    },
                    "required": ["date", "time", "patient_name"]
                }),
            },
        ]
    }

    /// The system prompt.
    pub fn prompt(&self, mode: ToolMode) -> String {
        let days: Vec<String> = self.days.iter().map(|d| d.format("%A %Y-%m-%d").to_string()).collect();
        let mut p = format!(
            "You are the front desk assistant of {CLINIC}, a small family doctor's office, answering a \
             phone call. Today is {today}. The clinic is open Monday to Friday, 9 AM to 5 PM, closed \
             12 to 1 for lunch; appointments are 30 minutes. Days you can book: {days}.\n\n\
             This is a voice call: your words are spoken aloud by a text-to-speech voice.\n\
             - Answer in one or two short sentences. Never use lists, markdown, emojis or abbreviations.\n\
             - Say times the way people say them: \"nine thirty in the morning\", \"two PM\".\n\
             - Offer at most three times at once.\n\
             - Never guess which times are open: check with check_availability first.\n\
             - Before booking, you need the day, the time and the caller's full name. As soon as you have \
               all three, call book_appointment. Never say an appointment is booked or scheduled before \
               book_appointment has answered; then confirm the day and time and give the confirmation code.\n\
             - Only pass a reason if the caller gave one.\n\
             - You cannot give medical advice. For an emergency, tell them to call 911.\n\
             - If something was cut off or unclear, ask them to repeat it.",
            today = self.today.format("%A %Y-%m-%d"),
            days = days.join(", "),
        );
        if mode == ToolMode::Json {
            p.push_str(
                "\n\nTools: to use one, reply with ONLY a JSON object and nothing else, e.g.\n\
                 {\"tool\": \"check_availability\", \"arguments\": {\"date\": \"2026-11-16\", \"part_of_day\": \"morning\"}}\n\
                 {\"tool\": \"book_appointment\", \"arguments\": {\"date\": \"2026-11-16\", \"time\": \"09:30\", \
                 \"patient_name\": \"Jane Doe\", \"reason\": \"check-up\"}}\n\
                 You then get the result in a message starting with TOOL RESULT, and answer the caller. \
                 When you are not using a tool, reply with plain spoken words, never JSON.",
            );
        }
        p
    }

    /// Runs one tool call; the result is JSON for the model.
    pub fn call(&mut self, call: &ToolCall) -> String {
        let result = match call.name.as_str() {
            "check_availability" => self.check(&call.arguments),
            "book_appointment" => self.book(&call.arguments),
            other => Err(format!(
                "There is no tool called {other:?}. The tools are check_availability and book_appointment."
            )),
        };
        match result {
            Ok(v) => v.to_string(),
            Err(e) => json!({"error": e}).to_string(),
        }
    }

    fn day(&self, args: &Value) -> Result<NaiveDate, String> {
        let text = arg(args, "date")?;
        let date = parse_date(&text, self.today).ok_or_else(|| {
            format!(
                "{text:?} is not a date. Give it as YYYY-MM-DD; today is {}.",
                self.today
            )
        })?;
        if !is_open_day(date) {
            return Err(format!(
                "The clinic is closed on {}s. The next open day is {}.",
                date.format("%A"),
                self.next_open(date)
                    .map_or("not in the calendar".into(), |d| d.format("%A %Y-%m-%d").to_string())
            ));
        }
        if !self.days.contains(&date) {
            return Err(format!(
                "{} can't be booked: only {} to {} can.",
                date.format("%A %Y-%m-%d"),
                self.days.first().map_or(String::new(), |d| d.to_string()),
                self.days.last().map_or(String::new(), |d| d.to_string()),
            ));
        }
        Ok(date)
    }

    fn next_open(&self, after: NaiveDate) -> Option<NaiveDate> {
        self.days.iter().copied().find(|d| *d > after)
    }

    fn is_free(&self, date: NaiveDate, time: NaiveTime) -> bool {
        !self.taken.contains(&(date, time)) && !self.bookings.iter().any(|b| b.date == date && b.time == time)
    }

    fn open_times(&self, date: NaiveDate) -> Vec<NaiveTime> {
        SLOTS
            .iter()
            .map(|(h, m)| slot(*h, *m))
            .filter(|t| self.is_free(date, *t))
            .collect()
    }

    fn check(&self, args: &Value) -> Result<Value, String> {
        let date = self.day(args)?;
        let part = args.get("part_of_day").and_then(Value::as_str).unwrap_or("any");
        let noon = slot(12, 0);
        let open: Vec<String> = self
            .open_times(date)
            .into_iter()
            .filter(|t| match part {
                "morning" => *t < noon,
                "afternoon" => *t >= noon,
                _ => true,
            })
            .map(|t| t.format("%H:%M").to_string())
            .collect();
        Ok(json!({"date": date.to_string(), "weekday": date.format("%A").to_string(), "open": open}))
    }

    fn book(&mut self, args: &Value) -> Result<Value, String> {
        let date = self.day(args)?;
        let text = arg(args, "time")?;
        let time = parse_time(&text).ok_or_else(|| format!("{text:?} is not a time. Give it as HH:MM, e.g. 09:30."))?;
        if !SLOTS.iter().any(|(h, m)| slot(*h, *m) == time) {
            return Err(format!(
                "{} is not an appointment time: they start on the hour and half hour, 9:00 to 16:30, not 12:00-13:00.",
                time.format("%H:%M")
            ));
        }
        if !self.is_free(date, time) {
            let others: Vec<String> = self
                .open_times(date)
                .iter()
                .map(|t| t.format("%H:%M").to_string())
                .collect();
            return Err(format!(
                "{} on {} is already taken. Open that day: {}.",
                time.format("%H:%M"),
                date,
                if others.is_empty() {
                    "nothing".into()
                } else {
                    others.join(", ")
                }
            ));
        }
        let name = arg(args, "patient_name")?;
        if name.split_whitespace().count() == 0 {
            return Err("Ask the caller for their name first.".into());
        }
        let reason = args.get("reason").and_then(Value::as_str).unwrap_or("").to_string();
        let code = format!("MS-{}", 1041 + self.bookings.len());
        self.bookings.push(Booking {
            date,
            time,
            name: name.clone(),
            reason,
            code: code.clone(),
        });
        Ok(json!({
            "booked": true,
            "confirmation_code": code,
            "date": date.to_string(),
            "weekday": date.format("%A").to_string(),
            "time": time.format("%H:%M").to_string(),
            "patient_name": name,
        }))
    }

    /// The bookings made during the call, one line each.
    pub fn booking_lines(&self) -> Vec<String> {
        self.bookings
            .iter()
            .map(|b| {
                let reason = if b.reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", b.reason)
                };
                format!(
                    "{} {} {}, {}{}",
                    b.code,
                    b.date.format("%a %Y-%m-%d"),
                    b.time.format("%H:%M"),
                    b.name,
                    reason
                )
            })
            .collect()
    }
}

fn arg(args: &Value, key: &str) -> Result<String, String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ if !args.is_object() => Err(format!("The arguments must be a JSON object, not {args}.")),
        _ => Err(format!("{key} is missing.")),
    }
}

/// `2026-11-16`, `today`, `tomorrow`, `monday`, `next monday`.
pub fn parse_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let t = text.trim().to_lowercase();
    if let Ok(d) = NaiveDate::parse_from_str(&t, "%Y-%m-%d") {
        return Some(d);
    }
    match t.as_str() {
        "today" => return Some(today),
        "tomorrow" => return Some(today + Duration::days(1)),
        _ => {}
    }
    let name = t.trim_start_matches("next ").trim_start_matches("this ");
    let weekday: Weekday = name.parse().ok()?;
    (1..=7)
        .map(|i| today + Duration::days(i))
        .find(|d| d.weekday() == weekday)
}

/// `09:30`, `9:30`, `9`, `9 am`, `9:30 AM`, `2pm`, `14:00`.
pub fn parse_time(text: &str) -> Option<NaiveTime> {
    let t = text.trim().to_lowercase().replace('.', "");
    let (t, pm) = if let Some(rest) = t.strip_suffix("pm") {
        (rest.trim().to_string(), Some(true))
    } else if let Some(rest) = t.strip_suffix("am") {
        (rest.trim().to_string(), Some(false))
    } else {
        (t, None)
    };
    let (h, m) = match t.split_once(':') {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?),
        None => (t.parse::<u32>().ok()?, 0),
    };
    let h = match (pm, h) {
        (Some(true), 1..=11) => h + 12,
        (Some(false), 12) => 0,
        (Some(_), 13..) => return None,
        _ => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

/// What a reply in JSON tool mode turned out to be.
#[derive(Debug, PartialEq)]
pub enum TextReply {
    /// Words for the caller.
    Speech,
    Call(ToolCall),
    /// Meant as a tool call, but it can't be read: ask again.
    Malformed(String),
}

/// Whether the start of a reply looks like JSON (or a code block): such a
/// reply is held back from the voice until it is complete.
pub fn looks_like_json(start: &str) -> bool {
    let s = start.trim_start();
    s.starts_with('{') || s.starts_with("```")
}

/// Reads a reply that may be a tool call written as JSON:
/// `{"tool": "...", "arguments": {...}}`, also with `name` for `tool` and
/// `parameters` for `arguments`, or wrapped in a ```json block.
pub fn parse_text_reply(text: &str, id: &str) -> TextReply {
    if !looks_like_json(text) {
        return TextReply::Speech;
    }
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return TextReply::Malformed("the JSON object is not closed".into());
    };
    if end < start {
        return TextReply::Malformed("the JSON object is not closed".into());
    }
    let v: Value = match serde_json::from_str(&text[start..=end]) {
        Ok(v) => v,
        Err(e) => return TextReply::Malformed(format!("it is not valid JSON ({e})")),
    };
    let v = v.get("function").cloned().unwrap_or(v);
    let name = ["tool", "name", "tool_name"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str));
    let Some(name) = name else {
        return TextReply::Malformed("it has no \"tool\" field".into());
    };
    let arguments = ["arguments", "parameters", "args", "input"]
        .iter()
        .find_map(|k| v.get(*k).cloned())
        .unwrap_or_else(|| json!({}));
    let arguments = match arguments {
        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        other => other,
    };
    TextReply::Call(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Thursday.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 12).unwrap()
    }

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments,
        }
    }

    fn result(clinic: &mut Clinic, name: &str, arguments: Value) -> Value {
        serde_json::from_str(&clinic.call(&call(name, arguments))).unwrap()
    }

    #[test]
    fn dates_and_times_in_the_ways_models_write_them() {
        let t = today();
        assert_eq!(parse_date("2026-11-16", t), NaiveDate::from_ymd_opt(2026, 11, 16));
        assert_eq!(parse_date("Tomorrow", t), NaiveDate::from_ymd_opt(2026, 11, 13));
        assert_eq!(parse_date("monday", t), NaiveDate::from_ymd_opt(2026, 11, 16));
        assert_eq!(parse_date("next Thursday", t), NaiveDate::from_ymd_opt(2026, 11, 19));
        assert_eq!(parse_date("soon", t), None);
        assert_eq!(parse_time("09:30"), Some(slot(9, 30)));
        assert_eq!(parse_time("9:30 AM"), Some(slot(9, 30)));
        assert_eq!(parse_time("2pm"), Some(slot(14, 0)));
        assert_eq!(parse_time("2:30 p.m."), Some(slot(14, 30)));
        assert_eq!(parse_time("12 pm"), Some(slot(12, 0)));
        assert_eq!(parse_time("14:00"), Some(slot(14, 0)));
        assert_eq!(parse_time("15pm"), None);
        assert_eq!(parse_time("half past"), None);
    }

    #[test]
    fn availability_is_the_same_every_run_and_skips_weekends() {
        let mut a = Clinic::demo(today());
        let mut b = Clinic::demo(today());
        let args = json!({"date": "2026-11-16"});
        assert_eq!(
            result(&mut a, "check_availability", args.clone()),
            result(&mut b, "check_availability", args)
        );
        let r = result(&mut a, "check_availability", json!({"date": "2026-11-16"}));
        let open = r["open"].as_array().unwrap();
        assert!(!open.is_empty() && open.len() < SLOTS.len(), "{r}");
        let morning = result(
            &mut a,
            "check_availability",
            json!({"date": "2026-11-16", "part_of_day": "morning"}),
        );
        assert!(
            morning["open"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t.as_str().unwrap() < "12:00")
        );
        let saturday = result(&mut a, "check_availability", json!({"date": "2026-11-14"}));
        assert!(
            saturday["error"].as_str().unwrap().contains("closed on Saturdays"),
            "{saturday}"
        );
        let far = result(&mut a, "check_availability", json!({"date": "2027-01-01"}));
        assert!(far["error"].as_str().unwrap().contains("can't be booked"), "{far}");
    }

    #[test]
    fn booking_takes_the_time_and_checks_every_argument() {
        let mut c = Clinic::demo(today());
        let r = result(&mut c, "check_availability", json!({"date": "2026-11-16"}));
        let free = r["open"][0].as_str().unwrap().to_string();
        let ok = result(
            &mut c,
            "book_appointment",
            json!({"date": "2026-11-16", "time": free, "patient_name": "Jane Doe", "reason": "check-up"}),
        );
        assert_eq!(ok["booked"], json!(true), "{ok}");
        assert_eq!(ok["confirmation_code"], "MS-1041");
        assert_eq!(c.booking_lines().len(), 1);

        let again = result(
            &mut c,
            "book_appointment",
            json!({"date": "2026-11-16", "time": free, "patient_name": "John Roe"}),
        );
        assert!(again["error"].as_str().unwrap().contains("already taken"), "{again}");
        let after = result(&mut c, "check_availability", json!({"date": "2026-11-16"}));
        assert!(!after["open"].as_array().unwrap().iter().any(|t| t == &json!(free)));

        let lunch = result(
            &mut c,
            "book_appointment",
            json!({"date": "2026-11-16", "time": "12:00", "patient_name": "J"}),
        );
        assert!(lunch["error"].as_str().unwrap().contains("not an appointment time"));
        let nameless = result(
            &mut c,
            "book_appointment",
            json!({"date": "2026-11-17", "time": "16:30"}),
        );
        assert!(nameless["error"].is_string(), "{nameless}");
        let junk = result(&mut c, "book_appointment", json!("09:00 monday"));
        assert!(junk["error"].as_str().unwrap().contains("JSON object"), "{junk}");
        let unknown = result(&mut c, "cancel_appointment", json!({}));
        assert!(unknown["error"].as_str().unwrap().contains("no tool"), "{unknown}");
    }

    #[test]
    fn tool_calls_written_as_json_text() {
        assert_eq!(parse_text_reply("Sure, nine thirty works.", "t"), TextReply::Speech);
        assert_eq!(
            parse_text_reply(
                r#" {"tool": "check_availability", "arguments": {"date": "2026-11-16"}}"#,
                "c"
            ),
            TextReply::Call(call("check_availability", json!({"date": "2026-11-16"})))
        );
        assert_eq!(
            parse_text_reply(
                "```json\n{\"name\": \"book_appointment\", \"parameters\": \"{\\\"time\\\": \\\"09:30\\\"}\"}\n```",
                "c"
            ),
            TextReply::Call(call("book_appointment", json!({"time": "09:30"})))
        );
        assert_eq!(
            parse_text_reply(r#"{"function": {"name": "check_availability"}}"#, "c"),
            TextReply::Call(call("check_availability", json!({})))
        );
        assert!(matches!(
            parse_text_reply(r#"{"tool": "check_availability", "arguments": {"date": "#, "t"),
            TextReply::Malformed(_)
        ));
        assert!(matches!(
            parse_text_reply(r#"{"date": "2026-11-16"}"#, "t"),
            TextReply::Malformed(_)
        ));
        assert!(looks_like_json("  {\"to"));
        assert!(looks_like_json("```"));
        assert!(!looks_like_json("Let me check {that}"));
    }

    #[test]
    fn the_prompt_lists_the_bookable_days_and_json_mode_explains_the_format() {
        let c = Clinic::demo(today());
        let native = c.prompt(ToolMode::Native);
        assert!(native.contains("Today is Thursday 2026-11-12"));
        assert!(native.contains("Friday 2026-11-13, Monday 2026-11-16"));
        assert!(!native.contains("Saturday"));
        assert!(!native.contains("TOOL RESULT"));
        assert!(c.prompt(ToolMode::Json).contains("TOOL RESULT"));
    }
}
