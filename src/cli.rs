//! The command line for people and AI agents: `doctor` (what the app sees
//! of this computer, and what looks wrong), `keys` (which keys the hotkey
//! watcher sees) and `settings` (read and change `settings.txt`). Plain
//! text by default, one JSON object per result with `--json`.

use std::io::Write as _;

use anyhow::{Context as _, Result, bail};

use crate::capture::annotate::Tool;
use crate::capture::files::Kind;
use crate::capture::record::Quality;
use crate::capture::scrolling::MAX_HEIGHTS;
use crate::dictation::hotkey::Shortcut;
use crate::dictation::ollama::json_string;
use crate::dictation::{self, Claim};
use crate::models::{self, ModelInfo};
use crate::platform::{self, KeyEvent, Press};
use crate::settings::{
    Choice, Clash, DELAYS, DrawStyle, OFF, ScrollStep, Settings, ShotAction, press_setting, settings_path,
};

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
        r.fact(
            "cleanup",
            format!("{} {} ({})", d.cleanup_api.id(), d.ollama_url, d.ollama_model),
        );
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

    // Screenshots.
    capture_facts(&mut r, &settings, &hotkey);

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

/// The capture hotkeys, where screenshots and screen recordings go and how
/// they are made, for `doctor`; clashes and unreadable hotkeys are problems.
fn capture_facts(r: &mut Report, settings: &Settings, dictation_hotkey: &Shortcut) {
    let c = &settings.capture;
    for press in Press::ALL {
        let fact = match press {
            Press::Screenshot => "screenshot_hotkey",
            Press::Scroll => "scroll_hotkey",
            Press::Record => "record_hotkey",
            Press::Pin => "pin_hotkey",
        };
        let key = c.press_hotkey(press);
        r.fact(
            fact,
            key.as_ref().map_or_else(
                || format!("off (setting {}: {})", press_setting(press), c.press_value(press)),
                |k| format!("{} (setting {}: {})", k.label(), press_setting(press), k.name()),
            ),
        );
        let label = press.label();
        if c.press_unreadable(press) {
            r.problems.push(format!(
                "The {label} hotkey setting {:?} is not a key or combination (or off); {} is used instead.",
                c.press_value(press),
                key.as_ref().map_or_else(|| "off".to_string(), |k| k.label())
            ));
        }
        match (c.clash(press, dictation_hotkey), &key) {
            (Some(Clash::Dictation), Some(k)) => r.problems.push(format!(
                "The {label} hotkey and the dictation hotkey are both {}: holding it dictates too.",
                k.label()
            )),
            (Some(Clash::Press(first)), Some(k)) => r.problems.push(format!(
                "The {label} hotkey is {}, the same as the {} hotkey, which goes first: it does nothing.",
                k.label(),
                first.label()
            )),
            _ => {}
        }
    }
    let meetings_dir = settings.meetings_dir_path();
    let goes = if c.into_meeting {
        "with no meeting open; else beside the meeting"
    } else {
        "always; not into meetings"
    };
    r.fact(
        "screenshots",
        crate::capture::unsupported().map_or_else(
            || format!("{} ({goes})", c.loose_dir(Kind::Screenshot, &meetings_dir).display()),
            str::to_string,
        ),
    );
    r.fact("screenshot_action", c.action.id());
    r.fact(
        "capture_delay",
        if c.delay == 0 {
            "none".to_string()
        } else {
            format!("{} seconds", c.delay)
        },
    );
    r.fact(
        "scrolling",
        format!(
            "step {}, at most {} px",
            c.scroll_step.id(),
            c.scroll_options().max_height()
        ),
    );
    let sound = c.sound();
    r.fact(
        "screen_recordings",
        crate::capture::record::unsupported().map_or_else(
            || {
                let heard = match (sound.computer, sound.microphone) {
                    (true, true) => "with the computer's sound and the microphone",
                    (true, false) => "with the computer's sound",
                    (false, true) => "with the microphone",
                    (false, false) => "without sound",
                };
                let pointer = if c.record_cursor {
                    "with the pointer"
                } else {
                    "without the pointer"
                };
                let quality = if cfg!(target_os = "macos") {
                    "quality chosen by the system".to_string()
                } else {
                    format!("quality {}", c.record_quality.id())
                };
                format!(
                    "{} ({goes}), {heard}, {pointer}, {} frames a second, {quality}",
                    c.loose_dir(Kind::Recording, &meetings_dir).display(),
                    c.record_fps
                )
            },
            str::to_string,
        ),
    );
}

/// The hotkey settings `pressed` is the keys of, each with what it is.
pub fn hotkey_settings(settings: &Settings, pressed: &Shortcut) -> Vec<(&'static str, &'static str)> {
    let mut found = Vec::new();
    if settings.dictation.hotkey() == *pressed {
        found.push(("dictation_hotkey", "dictation"));
    }
    for press in Press::ALL {
        if settings.capture.press_hotkey(press).as_ref() == Some(pressed) {
            found.push((press_setting(press), press.label()));
        }
    }
    found
}

/// The values a setting may take, when it is one of a few.
fn choices(key: &str) -> Option<Vec<String>> {
    fn ids<T: Choice>() -> Vec<String> {
        T::ALL.iter().map(|c| c.id().to_string()).collect()
    }
    let numbers = |list: &[u32]| list.iter().map(u32::to_string).collect();
    Some(match key {
        "screenshot_action" => ids::<ShotAction>(),
        "capture_delay" => numbers(&DELAYS),
        "draw_tool" => std::iter::once("none".to_string()).chain(ids::<Tool>()).collect(),
        "scroll_step" => ids::<ScrollStep>(),
        "record_fps" => numbers(&crate::capture::record::FPS_CHOICES),
        "record_quality" => ids::<Quality>(),
        _ => return None,
    })
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
    let examples = "Examples: right_ctrl, f13, ctrl+shift+vk:68. Run `meeting-transcriber keys` and press the \
                    keys to see their names.";
    if key == "dictation_hotkey" && Shortcut::parse(value).is_none() {
        bail!("{value:?} is not a key or combination. {examples}");
    }
    let press = Press::ALL.into_iter().any(|p| press_setting(p) == key);
    if press && !value.trim().eq_ignore_ascii_case(OFF) && Shortcut::parse(value).is_none() {
        bail!("{value:?} is not a key or combination, nor off. {examples}");
    }
    if let Some(choices) = choices(key)
        && !choices.iter().any(|c| c == value.trim())
    {
        bail!(
            "{value:?} is not a value for {key}; it is one of: {}",
            choices.join(", ")
        );
    }
    if key == "scroll_max_height"
        && value
            .trim()
            .parse::<u32>()
            .ok()
            .is_none_or(|n| !MAX_HEIGHTS.contains(&n))
    {
        bail!(
            "{value:?} is not a value for {key}: a number of pixels from {} to {}",
            MAX_HEIGHTS.start(),
            MAX_HEIGHTS.end()
        );
    }
    let changed = Settings::parse(&format!("{}\n{key}={value}\n", settings.to_text()));
    if key == "draw_style" {
        // Tools left out go back to their default; the rest is checked here.
        DrawStyle::check(value).map_err(|why| anyhow::anyhow!("{why}"))?;
        return Ok(changed);
    }
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
        let found = hotkey_settings(&settings, &pressed);
        let is_screenshot = found.iter().any(|(key, _)| *key == press_setting(Press::Screenshot));
        if json {
            let keys: Vec<String> = found.iter().map(|(key, _)| json_string(key)).collect();
            println!(
                "{{\"label\":{},\"name\":{},\"is_hotkey\":{is_hotkey},\"is_screenshot_hotkey\":{is_screenshot},\
                 \"settings\":[{}]}}",
                json_string(&pressed.label()),
                json_string(&pressed.name()),
                keys.join(",")
            );
        } else {
            let marks: Vec<String> = found
                .iter()
                .map(|(key, what)| format!("the {what} hotkey ({key})"))
                .collect();
            let mark = if marks.is_empty() {
                String::new()
            } else {
                format!("   <- {}", marks.join(", "))
            };
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

/// `gpu`: where Whisper runs and the CUDA add-on's state. `gpu install-cuda
/// [--from URL_OR_ZIP]`: downloads (or unpacks) the CUDA add-on.
pub fn run_gpu(args: &[String], json: bool) -> Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use crate::cuda_addon;

    match args {
        [] => {
            let mut r = Report::default();
            for (key, value) in crate::gpu::report() {
                r.fact(key, value);
            }
            if let Some(source) = cuda_addon::source() {
                r.fact("cuda_addon_source", source);
            }
            if cuda_addon::state().offer() {
                r.problems.push(
                    "An NVIDIA card is here without CUDA support: `meeting-transcriber gpu install-cuda` downloads it."
                        .into(),
                );
            }
            if json {
                println!("{}", r.to_json());
            } else {
                print!("{}", r.to_text());
            }
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "install-cuda" => {
            let from = match rest {
                [] => None,
                [flag, value] if flag == "--from" => Some(value.as_str()),
                _ => bail!("install-cuda takes only --from URL_OR_ZIP"),
            };
            let mut shown = 0;
            let folder = cuda_addon::install(from, Arc::new(AtomicBool::new(false)), |done, total| {
                let mb = done / 1_000_000;
                if mb >= shown + 25 {
                    shown = mb;
                    match total {
                        Some(t) => eprintln!("{mb} of {} MB", t / 1_000_000),
                        None => eprintln!("{mb} MB"),
                    }
                }
            })?;
            println!(
                "CUDA support is in {}. It is used once the app is opened again.",
                folder.display()
            );
            Ok(())
        }
        _ => bail!("gpu takes nothing, or install-cuda [--from URL_OR_ZIP]"),
    }
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
        let shot = set_setting(&s, "capture_hotkey", "super+shift+vk:20").unwrap();
        assert_eq!(shot.capture.hotkey, "super+shift+vk:20");
        assert!(set_setting(&s, "capture_hotkey", "ctrl+shift").is_err());
        // The dictation hotkey can't be off; the capture hotkeys can.
        assert!(set_setting(&s, "dictation_hotkey", "off").is_err());
        for key in ["capture_hotkey", "scroll_hotkey", "record_hotkey", "pin_hotkey"] {
            let off = set_setting(&s, key, "off").unwrap();
            assert_eq!(
                off.capture.press_hotkey(Press::ALL[0]).is_none(),
                key == "capture_hotkey"
            );
            let on = set_setting(&s, key, "ctrl+alt+vk:83").unwrap();
            assert!(settings_entries(&on).contains(&(key.to_string(), "ctrl+alt+vk:83".to_string())));
            assert!(set_setting(&s, key, "nonsense").is_err(), "{key}");
        }
        assert!(set_setting(&s, "no_such_setting", "1").is_err());
        assert!(set_setting(&s, "dictation_min_seconds", "abc").is_err());
        assert!(set_setting(&s, "language", "a\nb").is_err());
        let lang = set_setting(&s, "language", "zh").unwrap();
        assert_eq!(lang.language, "zh");
    }

    #[test]
    fn capture_settings_are_checked_when_set() {
        let s = Settings::default();
        let ok = |key: &str, value: &str| set_setting(&s, key, value).unwrap_or_else(|e| panic!("{key}={value}: {e}"));
        let bad = |key: &str, value: &str| assert!(set_setting(&s, key, value).is_err(), "{key}={value}");
        assert_eq!(ok("screenshot_action", "pin").capture.action, ShotAction::Pin);
        assert_eq!(ok("screenshot_action", "copy").capture.action, ShotAction::Copy);
        bad("screenshot_action", "print");
        assert_eq!(ok("capture_delay", "5").capture.delay, 5);
        bad("capture_delay", "4");
        bad("capture_delay", "-1");
        assert_eq!(ok("draw_tool", "mosaic").capture.draw_tool, Some(Tool::Mosaic));
        assert_eq!(ok("draw_tool", "none").capture.draw_tool, None);
        bad("draw_tool", "crayon");
        assert_eq!(ok("scroll_step", "large").capture.scroll_step, ScrollStep::Large);
        bad("scroll_step", "huge");
        assert_eq!(ok("scroll_max_height", "45000").capture.scroll_max_height, 45_000);
        bad("scroll_max_height", "100");
        bad("scroll_max_height", "999999");
        bad("scroll_max_height", "tall");
        assert_eq!(ok("record_fps", "60").capture.record_fps, 60);
        bad("record_fps", "24");
        assert_eq!(ok("record_quality", "high").capture.record_quality, Quality::High);
        bad("record_quality", "ultra");
        assert!(!ok("record_cursor", "false").capture.record_cursor);
        bad("record_cursor", "no");
        assert!(!ok("capture_into_meeting", "false").capture.into_meeting);
        bad("capture_into_meeting", "maybe");
        assert_eq!(ok("screenshots_dir", "~/Shots").capture.screenshots_dir, "~/Shots");
        assert_eq!(
            ok("recordings_dir", "/Volumes/X/Videos").capture.recordings_dir,
            "/Volumes/X/Videos"
        );
        let style = ok("draw_style", "pen:blue:large").capture.draw_style;
        assert_eq!(style.get(Tool::Pen).color, 3);
        assert_eq!(style.get(Tool::Arrow), crate::settings::ToolStyle::default());
        bad("draw_style", "pen:purple:large");
        bad("draw_style", "pen");
    }

    #[test]
    fn a_combination_is_told_which_hotkey_settings_it_is() {
        let mut s = Settings::default();
        let shot = crate::capture::default_hotkey();
        assert_eq!(hotkey_settings(&s, &shot), [("capture_hotkey", "screenshot")]);
        assert_eq!(
            hotkey_settings(&s, &s.dictation.hotkey()),
            [("dictation_hotkey", "dictation")]
        );
        s.capture.pin_hotkey = shot.name();
        s.capture.record_hotkey = "f13".into();
        assert_eq!(
            hotkey_settings(&s, &shot),
            [("capture_hotkey", "screenshot"), ("pin_hotkey", "pin")]
        );
        assert_eq!(
            hotkey_settings(&s, &Shortcut::parse("f13").unwrap()),
            [("record_hotkey", "screen recording")]
        );
        assert!(hotkey_settings(&s, &Shortcut::parse("f19").unwrap()).is_empty());
        // Off is no keys at all.
        s.capture.hotkey = OFF.into();
        assert_eq!(hotkey_settings(&s, &shot), [("pin_hotkey", "pin")]);
    }

    #[test]
    fn doctor_flags_capture_hotkey_clashes() {
        let mut s = Settings::default();
        s.capture.pin_hotkey = s.capture.hotkey.clone();
        s.capture.scroll_hotkey = "nonsense".into();
        s.capture.record_hotkey = s.dictation.hotkey().name();
        let mut r = Report::default();
        capture_facts(&mut r, &s, &s.dictation.hotkey());
        let fact = |key: &str| r.facts.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap();
        assert!(fact("scroll_hotkey").starts_with("off"), "{}", fact("scroll_hotkey"));
        assert!(fact("pin_hotkey").contains("pin_hotkey"));
        assert_eq!(fact("screenshot_action"), "copy_and_save");
        assert_eq!(fact("capture_delay"), "none");
        let problems = r.problems.join("\n");
        assert!(problems.contains("The pin hotkey is"), "{problems}");
        assert!(
            problems.contains("The scrolling screenshot hotkey setting \"nonsense\""),
            "{problems}"
        );
        assert!(
            problems.contains("The screen recording hotkey and the dictation hotkey"),
            "{problems}"
        );
        assert_eq!(r.problems.len(), 3, "{problems}");
        // The defaults are fine.
        let mut r = Report::default();
        let d = Settings::default();
        capture_facts(&mut r, &d, &d.dictation.hotkey());
        assert!(r.problems.is_empty(), "{:?}", r.problems);
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
