//! Remembers the user's choices between launches in a small key=value file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::dictation::DictationConfig;
use crate::dictation::hotkey::Hotkey;
use crate::dictation::ollama::{self, OllamaConfig};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub model: String,
    /// Empty for auto-detect.
    pub language: String,
    pub capture_me: bool,
    pub capture_others: bool,
    pub remove_echo: bool,
    /// Where models live. Empty for the default; may start with `~/`.
    pub models_dir: String,
    /// Where transcripts are saved. Empty for the default; may start with `~/`.
    pub meetings_dir: String,
    /// The old models folder while models are being moved out of it; empty
    /// otherwise. Saved before the move starts, so a move cut short (the
    /// app quit, the Mac slept) is finished on the next launch.
    pub models_move_from: String,
    pub dictation: DictationSettings,
}

/// Dictation's choices, with voice2cursor's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct DictationSettings {
    pub enabled: bool,
    /// A `Hotkey` name, e.g. `right_alt` or `vk:105`.
    pub hotkey: String,
    /// Empty for auto-detect.
    pub language: String,
    /// Steers Whisper's spelling (names, simplified Chinese); may be empty.
    pub initial_prompt: String,
    pub auto_paste: bool,
    pub sounds: bool,
    pub save_recordings: bool,
    /// Empty for the default (`archive::default_dir`); may start with `~/`.
    pub recordings_dir: String,
    pub min_seconds: f32,
    pub max_seconds: f32,
    pub ollama: bool,
    pub ollama_url: String,
    pub ollama_model: String,
    pub ollama_timeout: f32,
}

impl Default for DictationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: Hotkey::default().name(),
            language: String::new(),
            initial_prompt: String::new(),
            auto_paste: true,
            sounds: true,
            save_recordings: true,
            recordings_dir: String::new(),
            min_seconds: 0.35,
            max_seconds: 300.0,
            ollama: false,
            ollama_url: ollama::DEFAULT_URL.into(),
            ollama_model: ollama::DEFAULT_MODEL.into(),
            ollama_timeout: 15.0,
        }
    }
}

impl DictationSettings {
    /// Takes one `key=value` line of `settings.txt`, if it is dictation's.
    fn parse_line(&mut self, key: &str, value: &str) {
        let flag = value == "true";
        let number = |default: f32| value.parse::<f32>().ok().filter(|n| n.is_finite()).unwrap_or(default);
        match key {
            "dictation" => self.enabled = flag,
            "dictation_hotkey" => self.hotkey = value.to_string(),
            "dictation_language" => self.language = value.to_string(),
            "dictation_prompt" => self.initial_prompt = value.to_string(),
            "dictation_auto_paste" => self.auto_paste = flag,
            "dictation_sounds" => self.sounds = flag,
            "dictation_save_recordings" => self.save_recordings = flag,
            "dictation_recordings_dir" => self.recordings_dir = value.to_string(),
            "dictation_min_seconds" => self.min_seconds = number(self.min_seconds),
            "dictation_max_seconds" => self.max_seconds = number(self.max_seconds),
            "ollama" => self.ollama = flag,
            "ollama_url" => self.ollama_url = value.to_string(),
            "ollama_model" => self.ollama_model = value.to_string(),
            "ollama_timeout" => self.ollama_timeout = number(self.ollama_timeout),
            _ => {}
        }
    }

    pub fn hotkey(&self) -> Hotkey {
        Hotkey::parse(&self.hotkey).unwrap_or_default()
    }

    pub fn recordings_dir_path(&self) -> PathBuf {
        expand_dir(&self.recordings_dir).unwrap_or_else(crate::dictation::archive::default_dir)
    }

    /// What the dictation threads run with.
    pub fn config(&self) -> DictationConfig {
        let some = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        DictationConfig {
            enabled: self.enabled,
            language: some(&self.language),
            initial_prompt: some(&self.initial_prompt),
            auto_paste: self.auto_paste,
            sounds: self.sounds,
            save_recordings: self.save_recordings,
            recordings_dir: self.recordings_dir_path(),
            min_seconds: self.min_seconds,
            max_seconds: self.max_seconds,
            ollama: self.ollama.then(|| OllamaConfig {
                url: self.ollama_url.trim().to_string(),
                model: self.ollama_model.trim().to_string(),
                timeout: Duration::from_secs_f32(self.ollama_timeout.clamp(1.0, 600.0)),
            }),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: crate::models::DEFAULT_MODEL.id.to_string(),
            language: String::new(),
            capture_me: true,
            capture_others: true,
            remove_echo: true,
            models_dir: String::new(),
            meetings_dir: String::new(),
            models_move_from: String::new(),
            dictation: DictationSettings::default(),
        }
    }
}

/// `settings.txt` in the app data folder (see `models::app_support_dir`).
/// Not in the models folder, since that folder is itself a setting.
pub fn settings_path() -> PathBuf {
    crate::models::app_support_dir().join("settings.txt")
}

/// A folder setting as a path: None when empty (use the default), with a
/// leading `~/` expanded to the home folder.
pub fn expand_dir(value: &str) -> Option<PathBuf> {
    expand_with_home(value, dirs::home_dir().as_deref())
}

fn expand_with_home(value: &str, home: Option<&Path>) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    // Windows users may well write `~\Models`.
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    match (value.strip_prefix('~'), home) {
        (Some(""), Some(home)) => Some(home.to_path_buf()),
        (Some(rest), Some(home)) if rest.starts_with(separators) => {
            Some(home.join(rest.trim_start_matches(separators)))
        }
        _ => Some(PathBuf::from(value)),
    }
}

impl Settings {
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let flag = value == "true";
            match key.trim() {
                "model" => s.model = value.to_string(),
                "language" => s.language = value.to_string(),
                "capture_me" => s.capture_me = flag,
                "capture_others" => s.capture_others = flag,
                "remove_echo" => s.remove_echo = flag,
                "models_dir" => s.models_dir = value.to_string(),
                "meetings_dir" => s.meetings_dir = value.to_string(),
                "models_move_from" => s.models_move_from = value.to_string(),
                key => s.dictation.parse_line(key, value),
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let d = &self.dictation;
        // One line per value: a prompt pasted with line breaks keeps its words.
        let line = |s: &str| {
            s.split(['\r', '\n'])
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        format!(
            "model={}\nlanguage={}\ncapture_me={}\ncapture_others={}\nremove_echo={}\nmodels_dir={}\nmeetings_dir={}\nmodels_move_from={}\n\
             dictation={}\ndictation_hotkey={}\ndictation_language={}\ndictation_prompt={}\ndictation_auto_paste={}\n\
             dictation_sounds={}\ndictation_save_recordings={}\ndictation_recordings_dir={}\n\
             dictation_min_seconds={}\ndictation_max_seconds={}\n\
             ollama={}\nollama_url={}\nollama_model={}\nollama_timeout={}\n",
            self.model,
            self.language,
            self.capture_me,
            self.capture_others,
            self.remove_echo,
            self.models_dir,
            self.meetings_dir,
            self.models_move_from,
            d.enabled,
            d.hotkey,
            d.language,
            line(&d.initial_prompt),
            d.auto_paste,
            d.sounds,
            d.save_recordings,
            d.recordings_dir,
            d.min_seconds,
            d.max_seconds,
            d.ollama,
            line(&d.ollama_url),
            line(&d.ollama_model),
            d.ollama_timeout,
        )
    }

    /// The models folder chosen here, or None for the default.
    pub fn models_dir_path(&self) -> Option<PathBuf> {
        expand_dir(&self.models_dir)
    }

    /// The folder models are still being moved out of, if any.
    pub fn models_move_from_path(&self) -> Option<PathBuf> {
        expand_dir(&self.models_move_from)
    }

    /// The folder new transcripts are saved to.
    pub fn meetings_dir_path(&self) -> PathBuf {
        expand_dir(&self.meetings_dir).unwrap_or_else(crate::meetings::default_dir)
    }

    pub fn load() -> Self {
        std::fs::read_to_string(settings_path()).map_or_else(|_| Self::default(), |t| Self::parse(&t))
    }

    /// Writes through a temporary file, so a crash mid-write cannot leave
    /// settings that disagree with where the models are.
    pub fn save(&self) {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("txt.saving");
        if std::fs::write(&tmp, self.to_text())
            .and_then(|()| std::fs::rename(&tmp, &path))
            .is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults_for_missing_keys() {
        let s = Settings {
            model: "base".into(),
            language: "zh".into(),
            capture_me: false,
            capture_others: true,
            remove_echo: false,
            models_dir: "/Volumes/Big Disk/whisper=models".into(),
            meetings_dir: "~/Notes/Meetings".into(),
            models_move_from: "/Volumes/Old Disk/models".into(),
            dictation: DictationSettings {
                enabled: false,
                hotkey: "vk:105".into(),
                language: "zh".into(),
                initial_prompt: "以下是普通话的句子，使用简体中文。Names: Patrick=PS".into(),
                auto_paste: false,
                sounds: false,
                save_recordings: false,
                recordings_dir: "~/Dictation".into(),
                min_seconds: 0.5,
                max_seconds: 120.0,
                ollama: true,
                ollama_url: "http://127.0.0.1:11434/api/generate".into(),
                ollama_model: "qwen2.5:7b".into(),
                ollama_timeout: 7.5,
            },
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(
            Settings::parse("junk\nmodel=small"),
            Settings {
                model: "small".into(),
                ..Settings::default()
            }
        );
        // Files written before the folder settings existed.
        let old = Settings::parse("model=base\nlanguage=\ncapture_me=true\ncapture_others=true\nremove_echo=true\n");
        assert_eq!(old.models_dir, "");
        assert_eq!(old.models_dir_path(), None);
        assert_eq!(old.models_move_from_path(), None);
        // …and before dictation: voice2cursor's defaults.
        assert_eq!(old.dictation, DictationSettings::default());
        assert_eq!(old.dictation.hotkey(), Hotkey::default());
        let cfg = old.dictation.config();
        assert_eq!((cfg.min_seconds, cfg.max_seconds), (0.35, 300.0));
        assert!(cfg.enabled && cfg.auto_paste && cfg.sounds && cfg.save_recordings);
        assert_eq!(cfg.ollama, None);
        assert_eq!(cfg.recordings_dir, crate::dictation::archive::default_dir());
    }

    #[test]
    fn dictation_values_are_checked_and_kept_on_one_line() {
        let s = Settings::parse(
            "dictation_hotkey=alt_r\ndictation_min_seconds=abc\ndictation_max_seconds=NaN\nollama=true",
        );
        assert_eq!(s.dictation.hotkey(), Hotkey::RightAlt);
        assert_eq!(s.dictation.min_seconds, 0.35);
        assert_eq!(s.dictation.max_seconds, 300.0);
        assert!(s.dictation.config().ollama.is_some());
        assert_eq!(
            Settings::parse("dictation_hotkey=nonsense").dictation.hotkey(),
            Hotkey::default()
        );
        let mut multi = Settings::default();
        multi.dictation.initial_prompt = "first line\nsecond=line\r\n".into();
        let back = Settings::parse(&multi.to_text());
        assert_eq!(back.dictation.initial_prompt, "first line second=line");
        assert_eq!(
            back.dictation.config().initial_prompt.as_deref(),
            Some("first line second=line")
        );
        assert_eq!(Settings::default().dictation.config().initial_prompt, None);
    }

    #[test]
    fn folder_values_expand_home() {
        let home = Path::new("/Users/me");
        let expand = |v| expand_with_home(v, Some(home));
        assert_eq!(expand(""), None);
        assert_eq!(expand("   "), None);
        assert_eq!(expand("~"), Some(PathBuf::from("/Users/me")));
        assert_eq!(expand("~/Models"), Some(PathBuf::from("/Users/me/Models")));
        assert_eq!(expand("/Volumes/X/models"), Some(PathBuf::from("/Volumes/X/models")));
        // Only a leading "~/" is special.
        assert_eq!(expand("~other/x"), Some(PathBuf::from("~other/x")));
        assert_eq!(expand("/a/~/b"), Some(PathBuf::from("/a/~/b")));
        assert_eq!(expand_with_home("~/x", None), Some(PathBuf::from("~/x")));
        let backslash = expand(r"~\Models");
        if cfg!(windows) {
            assert_eq!(backslash, Some(home.join("Models")));
        } else {
            assert_eq!(backslash, Some(PathBuf::from(r"~\Models")));
        }
    }
}
