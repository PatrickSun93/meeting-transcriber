//! The command line for people and AI agents: `doctor` (what the app sees
//! of this computer, and what looks wrong), `keys` (which keys the hotkey
//! watcher sees) and `settings` (read and change `settings.txt`). Plain
//! text by default, one JSON object per result with `--json`.

use std::io::Write as _;

use anyhow::{Context as _, Result, bail};

use crate::dictation::hotkey::Shortcut;
use crate::dictation::ollama::json_string;
use crate::dictation::{self, Claim};
use crate::models::{self, ModelInfo};
use crate::platform::{self, KeyEvent};
use crate::settings::{Settings, settings_path};

/// What `doctor` found: facts in order, and what looks wrong.
#[derive(Debug, Default)]
pub struct Report {
    pub facts: Vec<(String, String)>,
    pub problems: Vec<String>,
}

impl Report {
    fn fact(&mut self, key: &str, value: impl Into<String>) {
        self.facts.push((key.into(), value.into()));
    }

    pub fn to_text(&self) -> String {
        let width = self.facts.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        let mut out = String::new();
        for (key, value) in &self.facts {
            out.push_str(&format!("{key:width$}  {value}\n"));
        }
        if self.problems.is_empty() {
            out.push_str("\nNo problems found.\n");
        } else {
            out.push_str("\nProblems:\n");
            for p in &self.problems {
                out.push_str(&format!("- {p}\n"));
            }
        }
        out
    }

    pub fn to_json(&self) -> String {
        let facts = self
            .facts
            .iter()
            .map(|(k, v)| format!("{}:{}", json_string(k), json_string(v)));
        let problems: Vec<String> = self.problems.iter().map(|p| json_string(p)).collect();
        let mut fields: Vec<String> = facts.collect();
        fields.push(format!("\"problems\":[{}]", problems.join(",")));
        format!("{{{}}}", fields.join(","))
    }
}

/// Looks at the settings, folders, models, hotkey and permissions.
pub fn doctor() -> Report {
    let mut r = Report::default();
    let settings = Settings::load();
    models::set_configured_dir(settings.models_dir_path());
    models::set_move_source(settings.models_move_from_path());

    r.fact("version", env!("CARGO_PKG_VERSION"));
    r.fact("os", format!("{} {}", std::env::consts::OS, std::env::consts::ARCH));
    if let Ok(exe) = std::env::current_exe() {
        r.fact("executable", exe.display().to_string());
    }
    r.fact("data_dir", models::app_support_dir().display().to_string());
    let log = crate::logfile::log_path();
    r.fact(
        "log_file",
        format!("{}{}", log.display(), if log.is_file() { "" } else { " (none yet)" }),
    );
    let path = settings_path();
    r.fact(
        "settings_file",
        format!(
            "{}{}",
            path.display(),
            if path.is_file() {
                ""
            } else {
                " (not saved yet: defaults)"
            }
        ),
    );

    // Meetings.
    let model = ModelInfo::by_id(&settings.model);
    r.fact("model", settings.model.clone());
    let models_dir = models::models_dir();
    r.fact("models_dir", models_dir.display().to_string());
    let downloaded: Vec<&str> = models::downloaded_in(&models_dir).iter().map(|(m, _)| m.id).collect();
    r.fact(
        "models_downloaded",
        if downloaded.is_empty() {
            "none".into()
        } else {
            downloaded.join(", ")
        },
    );
    match model {
        None => r
            .problems
            .push(format!("The model setting {:?} is not a known model.", settings.model)),
        Some(m) if !m.is_downloaded() => r.problems.push(format!(
            "The model {} is not downloaded to {}: nothing can be transcribed until it is (download it in Settings).",
            m.id,
            models_dir.display()
        )),
        Some(_) => {}
    }
    r.fact(
        "language",
        if settings.language.is_empty() {
            "auto-detect".into()
        } else {
            settings.language.clone()
        },
    );
    r.fact(
        "capture",
        match (settings.capture_me, settings.capture_others) {
            (true, true) => "Me + Others",
            (true, false) => "Me only",
            (false, true) => "Others only",
            (false, false) => "nothing",
        },
    );
    if !settings.capture_me && !settings.capture_others {
        r.problems
            .push("Neither Me nor Others is recorded: a meeting hears nothing.".into());
    }
    let meetings_dir = settings.meetings_dir_path();
    r.fact("meetings_dir", meetings_dir.display().to_string());

    // Dictation.
    let d = &settings.dictation;
    r.fact("dictation", if d.enabled { "on" } else { "off" });
    let hotkey = d.hotkey();
    r.fact("hotkey", format!("{} (setting: {})", hotkey.label(), hotkey.name()));
    if Shortcut::parse(&d.hotkey).is_none() {
        r.problems.push(format!(
            "The hotkey setting {:?} is not a key or combination; {} is used instead.",
            d.hotkey,
            hotkey.label()
        ));
    }
    r.fact("auto_paste", d.auto_paste.to_string());
    r.fact("recordings_dir", d.recordings_dir_path().display().to_string());
    if d.ollama {
        r.fact("ollama", format!("{} ({})", d.ollama_url, d.ollama_model));
    }
    let running = matches!(
        dictation::claim(&models::app_support_dir().join("dictation.lock")),
        Claim::Taken
    );
    r.fact("app_running", if running { "yes" } else { "no" });
    if cfg!(windows) {
        r.fact("installed", if platform::installed() { "yes" } else { "no" });
        if platform::running_from_temp() {
            r.problems.push(
                "The program runs from a temporary folder (opened from inside the zip): unzip it, or run \
                 `meeting-transcriber install`."
                    .into(),
            );
        }
    }

    // Where Whisper runs.
    for (key, value) in crate::gpu::report() {
        r.fact(key, value);
    }

    // What the system allows.
    for (key, value) in platform::diagnostics() {
        if value == "not allowed" {
            r.problems.push(format!(
                "{key} is not allowed for whatever ran this command (a terminal has permissions of its own; \
                 check the app itself in System Settings > Privacy & Security)."
            ));
        }
        r.fact(key, value);
    }
    r
}

/// Every setting as `settings.txt` writes it, in order.
pub fn settings_entries(settings: &Settings) -> Vec<(String, String)> {
    settings
        .to_text()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// `settings` with `key` set to `value`, checked: the key must exist and
/// the value must read back as given (a hotkey must be a key name, a flag
/// true or false, a number a number).
pub fn set_setting(settings: &Settings, key: &str, value: &str) -> Result<Settings> {
    let known = settings_entries(settings);
    if !known.iter().any(|(k, _)| k == key) {
        let keys: Vec<&str> = known.iter().map(|(k, _)| k.as_str()).collect();
        bail!("no setting {key:?}; the settings are: {}", keys.join(", "));
    }
    if value.contains(['\n', '\r']) {
        bail!("a setting is one line");
    }
    if key == "dictation_hotkey" && Shortcut::parse(value).is_none() {
        bail!(
            "{value:?} is not a key or combination. Examples: right_ctrl, f13, ctrl+shift+vk:68. \
             Run `meeting-transcriber keys` and press the keys to see their names."
        );
    }
    let changed = Settings::parse(&format!("{}\n{key}={value}\n", settings.to_text()));
    let now = settings_entries(&changed)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .unwrap_or_default();
    if now != value.trim() && now != value {
        bail!("{value:?} is not a valid value for {key} (it would read as {now:?})");
    }
    Ok(changed)
}

/// `settings`, `settings get KEY`, `settings set KEY VALUE`, `settings path`.
pub fn run_settings(args: &[String], json: bool) -> Result<()> {
    let settings = Settings::load();
    let print_entries = |entries: &[(String, String)]| {
        if json {
            let fields: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}:{}", json_string(k), json_string(v)))
                .collect();
            println!("{{{}}}", fields.join(","));
        } else {
            for (k, v) in entries {
                println!("{k}={v}");
            }
        }
    };
    match args {
        [] => print_entries(&settings_entries(&settings)),
        [cmd] if cmd == "path" => println!("{}", settings_path().display()),
        [cmd, key] if cmd == "get" => {
            let entry = settings_entries(&settings)
                .into_iter()
                .find(|(k, _)| k == key)
                .with_context(|| format!("no setting {key:?}"))?;
            if json {
                print_entries(&[entry]);
            } else {
                println!("{}", entry.1);
            }
        }
        [cmd, key, value @ ..] if cmd == "set" && !value.is_empty() => {
            let value = value.join(" ");
            let changed = set_setting(&settings, key, &value)?;
            changed.save();
            if !settings_path().is_file() {
                bail!("could not save {}", settings_path().display());
            }
            let entry = settings_entries(&changed).into_iter().find(|(k, _)| k == key);
            print_entries(&entry.into_iter().collect::<Vec<_>>());
            eprintln!("Saved. A running app picks this up when it is next opened.");
        }
        _ => bail!("settings takes nothing, `path`, `get KEY` or `set KEY VALUE`"),
    }
    Ok(())
}

/// Prints every key or combination held and let go of, as the dictation
/// hotkey watcher sees it, until Ctrl+C (or one, with `once`). With `save`,
/// saves the first one as the dictation hotkey.
pub fn run_keys(once: bool, save: bool, json: bool) -> Result<()> {
    let mut settings = Settings::load();
    let hotkey = settings.dictation.hotkey();
    let (tx, rx) = crossbeam_channel::unbounded();
    let watch = platform::watch_hotkey(hotkey.clone(), move |event| {
        let _ = tx.send(event);
    })
    .map_err(|e| anyhow::anyhow!("{e:#} {}", platform::HOTKEY_HINT))?;
    if let Some(label) = watch.system_key() {
        bail!(
            "This session does not let apps see the keyboard (Wayland, or no X11 display); dictation starts with {label}, \
             set in the desktop's settings, or `meeting-transcriber dictate toggle` bound to a shortcut."
        );
    }
    eprintln!(
        "Dictation hotkey: {} (setting: {}).\nHold any key or combination, then let go: each is printed. Ctrl+C quits.",
        hotkey.label(),
        hotkey.name()
    );
    watch.capture_next(true);
    for event in rx {
        let pressed = match event {
            KeyEvent::Captured(shortcut) => shortcut,
            // Escape alone ends capturing in the app; here it is a key.
            KeyEvent::CaptureCancelled => Shortcut::from(crate::dictation::hotkey::Hotkey::Vk(
                crate::dictation::hotkey::native_codes().escape,
            )),
            KeyEvent::Stopped => bail!("the keyboard watcher stopped. {}", platform::HOTKEY_HINT),
            _ => continue,
        };
        let is_hotkey = pressed == hotkey;
        if json {
            println!(
                "{{\"label\":{},\"name\":{},\"is_hotkey\":{is_hotkey}}}",
                json_string(&pressed.label()),
                json_string(&pressed.name())
            );
        } else {
            let mark = if is_hotkey { "   <- the dictation hotkey" } else { "" };
            println!("{}   (setting: {}){mark}", pressed.label(), pressed.name());
        }
        let _ = std::io::stdout().flush();
        if save {
            settings.dictation.hotkey = pressed.name();
            settings.save();
            eprintln!("Saved as the dictation hotkey. A running app picks this up when it is next opened.");
            return Ok(());
        }
        if once {
            return Ok(());
        }
        watch.capture_next(true);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_listed_and_set_with_checks() {
        let s = Settings::default();
        let entries = settings_entries(&s);
        assert!(entries.iter().any(|(k, _)| k == "dictation_hotkey"));
        let changed = set_setting(&s, "dictation_hotkey", "ctrl+shift+vk:68").unwrap();
        assert_eq!(changed.dictation.hotkey, "ctrl+shift+vk:68");
        assert!(set_setting(&s, "dictation_hotkey", "nonsense").is_err());
        assert!(set_setting(&s, "no_such_setting", "1").is_err());
        assert!(set_setting(&s, "dictation_min_seconds", "abc").is_err());
        assert!(set_setting(&s, "language", "a\nb").is_err());
        let lang = set_setting(&s, "language", "zh").unwrap();
        assert_eq!(lang.language, "zh");
    }

    #[test]
    fn reports_render_as_text_and_json() {
        let mut r = Report::default();
        r.fact("os", "windows x86_64");
        r.problems.push("a \"quoted\" problem".into());
        assert!(r.to_text().contains("os  windows x86_64"));
        assert!(r.to_text().contains("- a \"quoted\" problem"));
        assert_eq!(
            r.to_json(),
            r#"{"os":"windows x86_64","problems":["a \"quoted\" problem"]}"#
        );
    }
}
