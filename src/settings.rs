//! Remembers the user's choices between launches in a small key=value file.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub model: String,
    /// Empty for auto-detect.
    pub language: String,
    pub capture_me: bool,
    pub capture_others: bool,
    pub remove_echo: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: crate::models::DEFAULT_MODEL.id.to_string(),
            language: String::new(),
            capture_me: true,
            capture_others: true,
            remove_echo: true,
        }
    }
}

fn path() -> PathBuf {
    crate::models::models_dir()
        .parent()
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("settings.txt")
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
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        format!(
            "model={}\nlanguage={}\ncapture_me={}\ncapture_others={}\nremove_echo={}\n",
            self.model, self.language, self.capture_me, self.capture_others, self.remove_echo
        )
    }

    pub fn load() -> Self {
        std::fs::read_to_string(path()).map_or_else(|_| Self::default(), |t| Self::parse(&t))
    }

    pub fn save(&self) {
        let path = path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.to_text());
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
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(
            Settings::parse("junk\nmodel=small"),
            Settings {
                model: "small".into(),
                ..Settings::default()
            }
        );
    }
}
