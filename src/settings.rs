//! Remembers the user's choices between launches in a small key=value file.

use std::path::{Path, PathBuf};

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
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        format!(
            "model={}\nlanguage={}\ncapture_me={}\ncapture_others={}\nremove_echo={}\nmodels_dir={}\nmeetings_dir={}\nmodels_move_from={}\n",
            self.model,
            self.language,
            self.capture_me,
            self.capture_others,
            self.remove_echo,
            self.models_dir,
            self.meetings_dir,
            self.models_move_from
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
