//! Remembers the user's choices between launches in a small key=value file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::capture::annotate::Tool;
use crate::capture::files::Kind;
use crate::capture::record::{self, Quality};
use crate::capture::scrolling;
use crate::dictation::DictationConfig;
#[cfg(test)]
use crate::dictation::hotkey::Hotkey;
use crate::dictation::hotkey::Shortcut;
use crate::dictation::ollama::{self, Api, OllamaConfig};
use crate::platform::Press;

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
    /// Run Whisper on the graphics card when there is one (Windows: picks
    /// the CUDA or Vulkan build next to the program; see `gpu`).
    pub use_gpu: bool,
    pub dictation: DictationSettings,
    pub capture: CaptureSettings,
}

/// Screenshots' and screen recordings' choices.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureSettings {
    /// Each a `Shortcut` name, pressed (not held), or `off` (or empty):
    /// the screenshot hotkey (`capture_hotkey`)…
    pub hotkey: String,
    /// …a scrolling screenshot's, a screen recording's, a pin's.
    pub scroll_hotkey: String,
    pub record_hotkey: String,
    pub pin_hotkey: String,
    /// What Enter, a double-click or Copy does with a plain screenshot.
    pub action: ShotAction,
    /// Where screenshots and screen recordings go with no meeting open (or
    /// not into meetings). Empty for the Screenshots and Recordings folders
    /// in the meetings folder; may start with `~/`.
    pub screenshots_dir: String,
    pub recordings_dir: String,
    /// Into the meeting open in the window, with a line in its notes.
    pub into_meeting: bool,
    /// Seconds between asking for a screenshot and taking it: one of `DELAYS`.
    pub delay: u32,
    /// The drawing tool the overlay opens with, if any.
    pub draw_tool: Option<Tool>,
    /// Each tool's colour and size, as last used.
    pub draw_style: DrawStyle,
    pub scroll_step: ScrollStep,
    /// The longest a scrolling screenshot gets, in pixels (`scrolling::MAX_HEIGHTS`).
    pub scroll_max_height: u32,
    /// Frames a second: one of `record::FPS_CHOICES`.
    pub record_fps: u32,
    /// With the pointer in the video.
    pub record_cursor: bool,
    pub record_quality: Quality,
    /// What screen recordings hear: the computer's sound, the microphone.
    pub record_computer_sound: bool,
    pub record_microphone: bool,
}

/// The value of a hotkey setting that is switched off.
pub const OFF: &str = "off";

/// The seconds `capture_delay` may be.
pub const DELAYS: [u32; 4] = [0, 3, 5, 10];

/// A setting that is one of a few named values.
pub trait Choice: Copy + PartialEq + 'static {
    const ALL: &'static [Self];

    /// As `settings.txt` has it.
    fn id(self) -> &'static str;

    /// As Settings shows it.
    fn label(self) -> &'static str;

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.id() == id.trim())
    }
}

/// What Enter, a double-click or Copy does with a plain screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShotAction {
    /// On the clipboard, and saved (with a line in the notes).
    #[default]
    CopyAndSave,
    /// Only on the clipboard: no file, no line.
    Copy,
    /// Only saved.
    Save,
    /// Pinned to the screen.
    Pin,
}

impl ShotAction {
    /// Whether Copy saves the picture too.
    pub fn copy_saves(self) -> bool {
        self != ShotAction::Copy
    }
}

impl Choice for ShotAction {
    const ALL: &'static [Self] = &[
        ShotAction::CopyAndSave,
        ShotAction::Copy,
        ShotAction::Save,
        ShotAction::Pin,
    ];

    fn id(self) -> &'static str {
        match self {
            ShotAction::CopyAndSave => "copy_and_save",
            ShotAction::Copy => "copy",
            ShotAction::Save => "save",
            ShotAction::Pin => "pin",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ShotAction::CopyAndSave => "Copy and save",
            ShotAction::Copy => "Copy only (no file)",
            ShotAction::Save => "Save only",
            ShotAction::Pin => "Pin to the screen",
        }
    }
}

/// How far a scrolling screenshot's first turn of the wheel goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollStep {
    Small,
    #[default]
    Normal,
    Large,
}

impl ScrollStep {
    /// As a part of the area's height.
    pub fn fraction(self) -> f32 {
        match self {
            ScrollStep::Small => 0.25,
            ScrollStep::Normal => 0.5,
            ScrollStep::Large => 0.75,
        }
    }
}

impl Choice for ScrollStep {
    const ALL: &'static [Self] = &[ScrollStep::Small, ScrollStep::Normal, ScrollStep::Large];

    fn id(self) -> &'static str {
        match self {
            ScrollStep::Small => "small",
            ScrollStep::Normal => "normal",
            ScrollStep::Large => "large",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ScrollStep::Small => "Small (a quarter of the area)",
            ScrollStep::Normal => "Normal (half the area)",
            ScrollStep::Large => "Large (three quarters)",
        }
    }
}

impl Choice for Quality {
    const ALL: &'static [Self] = &Quality::ALL;

    fn id(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::Normal => "normal",
            Quality::High => "high",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Quality::Low => "Low (smaller files)",
            Quality::Normal => "Normal",
            Quality::High => "High (larger files)",
        }
    }
}

impl Choice for Tool {
    const ALL: &'static [Self] = &Tool::ALL;

    fn id(self) -> &'static str {
        match self {
            Tool::Rectangle => "rectangle",
            Tool::Ellipse => "ellipse",
            Tool::Arrow => "arrow",
            Tool::Pen => "pen",
            Tool::Text => "text",
            Tool::Mosaic => "mosaic",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tool::Rectangle => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Arrow => "Arrow",
            Tool::Pen => "Pen",
            Tool::Text => "Text",
            Tool::Mosaic => "Mosaic",
        }
    }
}

/// The overlay's colours and sizes by name, in the toolbar's order.
pub const DRAW_COLORS: [&str; 6] = ["red", "yellow", "green", "blue", "white", "black"];
pub const DRAW_SIZES: [&str; 3] = ["small", "medium", "large"];

/// A drawing tool's colour and size: places in `DRAW_COLORS` and `DRAW_SIZES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolStyle {
    pub color: usize,
    pub size: usize,
}

impl Default for ToolStyle {
    fn default() -> Self {
        Self { color: 0, size: 1 }
    }
}

/// Each drawing tool's colour and size, by `Tool::index`. Saved as
/// `rectangle:red:medium ellipse:blue:large …`; a part that can't be read
/// keeps its tool's default (red, medium).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DrawStyle {
    pub tools: [ToolStyle; 6],
}

impl DrawStyle {
    pub fn get(&self, tool: Tool) -> ToolStyle {
        self.tools[tool.index()]
    }

    pub fn set(&mut self, tool: Tool, style: ToolStyle) {
        self.tools[tool.index()] = style;
    }

    /// Reads the saved form, leniently: what can't be read is left at its default.
    pub fn parse(value: &str) -> Self {
        let mut style = Self::default();
        for (tool, tool_style) in value.split_whitespace().filter_map(|part| Self::part(part).ok()) {
            style.set(tool, tool_style);
        }
        style
    }

    /// Reads the saved form strictly (for `settings set`): why a part can't be read.
    pub fn check(value: &str) -> Result<Self, String> {
        let mut style = Self::default();
        for part in value.split_whitespace() {
            let (tool, tool_style) = Self::part(part)?;
            style.set(tool, tool_style);
        }
        Ok(style)
    }

    /// One `tool:colour:size`.
    fn part(part: &str) -> Result<(Tool, ToolStyle), String> {
        let bad = || {
            format!(
                "{part:?} is not tool:colour:size (tools: {}; colours: {}; sizes: {})",
                Tool::ALL.map(Choice::id).join(", "),
                DRAW_COLORS.join(", "),
                DRAW_SIZES.join(", ")
            )
        };
        let mut fields = part.split(':');
        let (Some(tool), Some(color), Some(size), None) = (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(bad());
        };
        let tool = Tool::from_id(tool).ok_or_else(bad)?;
        let color = DRAW_COLORS.iter().position(|c| *c == color).ok_or_else(bad)?;
        let size = DRAW_SIZES.iter().position(|s| *s == size).ok_or_else(bad)?;
        Ok((tool, ToolStyle { color, size }))
    }

    pub fn to_text(&self) -> String {
        Tool::ALL
            .iter()
            .map(|tool| {
                let ToolStyle { color, size } = self.get(*tool);
                let color = DRAW_COLORS.get(color).unwrap_or(&DRAW_COLORS[0]);
                let size = DRAW_SIZES.get(size).unwrap_or(&DRAW_SIZES[1]);
                format!("{}:{color}:{size}", tool.id())
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Which setting holds the hotkey of `press`.
pub fn press_setting(press: Press) -> &'static str {
    match press {
        Press::Screenshot => "capture_hotkey",
        Press::Scroll => "scroll_hotkey",
        Press::Record => "record_hotkey",
        Press::Pin => "pin_hotkey",
    }
}

/// The hotkey of `press` until one is chosen: the screenshot's, ⌘⇧2 or
/// Ctrl+Shift+2; the others are off.
pub fn default_press_hotkey(press: Press) -> Option<Shortcut> {
    (press == Press::Screenshot).then(crate::capture::default_hotkey)
}

/// A hotkey setting's value: None when off (`off`, or empty), the keys
/// when it names some, else `default` (a value that can't be read).
fn press_shortcut(value: &str, default: Option<Shortcut>) -> Option<Shortcut> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case(OFF) {
        return None;
    }
    Shortcut::parse(value).or(default)
}

/// Why a capture hotkey does not do what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clash {
    /// It is the dictation hotkey too: holding it dictates as well.
    Dictation,
    /// An earlier capture hotkey has the same keys, and wins.
    Press(Press),
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            hotkey: crate::capture::default_hotkey().name(),
            scroll_hotkey: OFF.into(),
            record_hotkey: OFF.into(),
            pin_hotkey: OFF.into(),
            action: ShotAction::default(),
            screenshots_dir: String::new(),
            recordings_dir: String::new(),
            into_meeting: true,
            delay: 0,
            draw_tool: None,
            draw_style: DrawStyle::default(),
            scroll_step: ScrollStep::default(),
            scroll_max_height: scrolling::MAX_HEIGHT,
            record_fps: record::DEFAULT_FPS,
            record_cursor: true,
            record_quality: Quality::default(),
            record_computer_sound: true,
            record_microphone: true,
        }
    }
}

impl CaptureSettings {
    /// Takes one `key=value` line of `settings.txt`, if it is screenshots'.
    fn parse_line(&mut self, key: &str, value: &str) {
        let flag = value == "true";
        let number = || value.parse::<u32>().ok();
        match key {
            "capture_hotkey" => self.hotkey = value.to_string(),
            "scroll_hotkey" => self.scroll_hotkey = value.to_string(),
            "record_hotkey" => self.record_hotkey = value.to_string(),
            "pin_hotkey" => self.pin_hotkey = value.to_string(),
            "screenshot_action" => self.action = ShotAction::from_id(value).unwrap_or_default(),
            "screenshots_dir" => self.screenshots_dir = value.to_string(),
            "recordings_dir" => self.recordings_dir = value.to_string(),
            "capture_into_meeting" => self.into_meeting = flag,
            "capture_delay" => self.delay = number().filter(|n| DELAYS.contains(n)).unwrap_or(0),
            "draw_tool" => self.draw_tool = Tool::from_id(value),
            "draw_style" => self.draw_style = DrawStyle::parse(value),
            "scroll_step" => self.scroll_step = ScrollStep::from_id(value).unwrap_or_default(),
            "scroll_max_height" => {
                self.scroll_max_height = number().map_or(scrolling::MAX_HEIGHT, |n| {
                    n.clamp(*scrolling::MAX_HEIGHTS.start(), *scrolling::MAX_HEIGHTS.end())
                });
            }
            "record_fps" => {
                self.record_fps = number()
                    .filter(|n| record::FPS_CHOICES.contains(n))
                    .unwrap_or(record::DEFAULT_FPS);
            }
            "record_cursor" => self.record_cursor = flag,
            "record_quality" => self.record_quality = Quality::from_id(value).unwrap_or_default(),
            "record_computer_sound" => self.record_computer_sound = flag,
            "record_microphone" => self.record_microphone = flag,
            _ => {}
        }
    }

    /// The `key=value` lines `settings.txt` gets.
    fn to_text(&self) -> String {
        let one_line = |s: &str| s.replace(['\r', '\n'], " ");
        format!(
            "capture_hotkey={}\nscroll_hotkey={}\nrecord_hotkey={}\npin_hotkey={}\nscreenshot_action={}\n\
             screenshots_dir={}\nrecordings_dir={}\ncapture_into_meeting={}\ncapture_delay={}\n\
             draw_tool={}\ndraw_style={}\nscroll_step={}\nscroll_max_height={}\n\
             record_fps={}\nrecord_cursor={}\nrecord_quality={}\nrecord_computer_sound={}\nrecord_microphone={}\n",
            one_line(&self.hotkey),
            one_line(&self.scroll_hotkey),
            one_line(&self.record_hotkey),
            one_line(&self.pin_hotkey),
            self.action.id(),
            one_line(&self.screenshots_dir),
            one_line(&self.recordings_dir),
            self.into_meeting,
            self.delay,
            self.draw_tool.map_or("none", Choice::id),
            self.draw_style.to_text(),
            self.scroll_step.id(),
            self.scroll_max_height,
            self.record_fps,
            self.record_cursor,
            self.record_quality.id(),
            self.record_computer_sound,
            self.record_microphone,
        )
    }

    /// What a screen recording hears.
    pub fn sound(&self) -> crate::capture::record::Sound {
        crate::capture::record::Sound {
            computer: self.record_computer_sound,
            microphone: self.record_microphone,
        }
    }

    /// How a screen recording is made.
    pub fn record_options(&self) -> record::Options {
        record::Options {
            sound: self.sound(),
            fps: self.record_fps,
            cursor: self.record_cursor,
            quality: self.record_quality,
        }
    }

    /// How a scrolling screenshot goes.
    pub fn scroll_options(&self) -> scrolling::Options {
        scrolling::Options {
            step: self.scroll_step.fraction(),
            max_height: self.scroll_max_height,
        }
    }

    /// The setting that holds the hotkey of `press`, as stored.
    pub fn press_value(&self, press: Press) -> &str {
        match press {
            Press::Screenshot => &self.hotkey,
            Press::Scroll => &self.scroll_hotkey,
            Press::Record => &self.record_hotkey,
            Press::Pin => &self.pin_hotkey,
        }
    }

    pub fn press_value_mut(&mut self, press: Press) -> &mut String {
        match press {
            Press::Screenshot => &mut self.hotkey,
            Press::Scroll => &mut self.scroll_hotkey,
            Press::Record => &mut self.record_hotkey,
            Press::Pin => &mut self.pin_hotkey,
        }
    }

    /// The hotkey of `press`, or None when it is off. A value that can't be
    /// read is the default.
    pub fn press_hotkey(&self, press: Press) -> Option<Shortcut> {
        press_shortcut(self.press_value(press), default_press_hotkey(press))
    }

    /// Whether the hotkey setting of `press` is neither off nor keys (and
    /// so its default is used instead).
    pub fn press_unreadable(&self, press: Press) -> bool {
        let value = self.press_value(press).trim();
        !value.is_empty() && !value.eq_ignore_ascii_case(OFF) && Shortcut::parse(value).is_none()
    }

    /// The hotkeys to watch, each with what it does: those switched on,
    /// and of two with the same keys only the first (`Press::ALL`'s order).
    pub fn presses(&self) -> Vec<(Press, Shortcut)> {
        let mut presses: Vec<(Press, Shortcut)> = Vec::new();
        for press in Press::ALL {
            if let Some(key) = self.press_hotkey(press)
                && !presses.iter().any(|(_, k)| *k == key)
            {
                presses.push((press, key));
            }
        }
        presses
    }

    /// Why the hotkey of `press` does not simply do its own thing, if it
    /// does not: it is the `dictation` hotkey too, or an earlier capture
    /// hotkey's keys.
    pub fn clash(&self, press: Press, dictation: &Shortcut) -> Option<Clash> {
        let key = self.press_hotkey(press)?;
        let earlier = Press::ALL
            .into_iter()
            .take_while(|p| *p != press)
            .find(|p| self.press_hotkey(*p).as_ref() == Some(&key));
        match earlier {
            Some(p) => Some(Clash::Press(p)),
            None => (key == *dictation).then_some(Clash::Dictation),
        }
    }

    /// The folder files of `kind` go to when not into a meeting: the one
    /// chosen, else its folder in `meetings_dir`.
    pub fn loose_dir(&self, kind: Kind, meetings_dir: &Path) -> PathBuf {
        let chosen = match kind {
            Kind::Screenshot => &self.screenshots_dir,
            Kind::Recording => &self.recordings_dir,
        };
        expand_dir(chosen).unwrap_or_else(|| meetings_dir.join(kind.loose_dir()))
    }
}

/// Dictation's choices, with voice2cursor's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct DictationSettings {
    pub enabled: bool,
    /// A `Shortcut` name: one key (`right_alt`, `vk:105`) or a combination
    /// (`ctrl+shift+vk:68`).
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
    /// Which kind of service the clean-up uses (the settings keep the
    /// `ollama` names from when Ollama was the only one).
    pub cleanup_api: Api,
    pub ollama_url: String,
    pub ollama_model: String,
    pub ollama_timeout: f32,
    /// Empty to read it from the environment (`Api::key_env`).
    pub cleanup_api_key: String,
}

impl Default for DictationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: Shortcut::default().name(),
            language: String::new(),
            initial_prompt: String::new(),
            auto_paste: true,
            sounds: true,
            save_recordings: true,
            recordings_dir: String::new(),
            min_seconds: 0.35,
            max_seconds: 300.0,
            ollama: false,
            cleanup_api: Api::Ollama,
            ollama_url: ollama::DEFAULT_URL.into(),
            ollama_model: ollama::DEFAULT_MODEL.into(),
            ollama_timeout: 15.0,
            cleanup_api_key: String::new(),
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
            "cleanup_api" => self.cleanup_api = Api::from_id(value).unwrap_or_default(),
            "cleanup_api_key" => self.cleanup_api_key = value.to_string(),
            _ => {}
        }
    }

    /// The key from Settings, else from the environment.
    fn cleanup_api_key(&self) -> String {
        let key = self.cleanup_api_key.trim();
        if !key.is_empty() {
            return key.to_string();
        }
        self.cleanup_api
            .key_env()
            .and_then(|name| std::env::var(name).ok())
            .map(|k| k.trim().to_string())
            .unwrap_or_default()
    }

    pub fn hotkey(&self) -> Shortcut {
        Shortcut::parse(&self.hotkey).unwrap_or_default()
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
                api: self.cleanup_api,
                url: self.ollama_url.trim().to_string(),
                model: self.ollama_model.trim().to_string(),
                api_key: self.cleanup_api_key(),
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
            use_gpu: true,
            models_dir: String::new(),
            meetings_dir: String::new(),
            models_move_from: String::new(),
            dictation: DictationSettings::default(),
            capture: CaptureSettings::default(),
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
                "use_gpu" => s.use_gpu = flag,
                key => {
                    s.dictation.parse_line(key, value);
                    s.capture.parse_line(key, value);
                }
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
            "model={}\nlanguage={}\ncapture_me={}\ncapture_others={}\nremove_echo={}\nmodels_dir={}\nmeetings_dir={}\nmodels_move_from={}\nuse_gpu={}\n\
             dictation={}\ndictation_hotkey={}\ndictation_language={}\ndictation_prompt={}\ndictation_auto_paste={}\n\
             dictation_sounds={}\ndictation_save_recordings={}\ndictation_recordings_dir={}\n\
             dictation_min_seconds={}\ndictation_max_seconds={}\n\
             ollama={}\ncleanup_api={}\nollama_url={}\nollama_model={}\nollama_timeout={}\ncleanup_api_key={}\n{}",
            self.model,
            self.language,
            self.capture_me,
            self.capture_others,
            self.remove_echo,
            self.models_dir,
            self.meetings_dir,
            self.models_move_from,
            self.use_gpu,
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
            d.cleanup_api.id(),
            line(&d.ollama_url),
            line(&d.ollama_model),
            d.ollama_timeout,
            line(&d.cleanup_api_key),
            self.capture.to_text(),
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
            use_gpu: false,
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
                cleanup_api: Api::Anthropic,
                ollama_url: "http://127.0.0.1:11434/api/generate".into(),
                ollama_model: "qwen2.5:7b".into(),
                ollama_timeout: 7.5,
                cleanup_api_key: "sk-test".into(),
            },
            capture: CaptureSettings {
                hotkey: "alt+shift+f13".into(),
                scroll_hotkey: "ctrl+shift+vk:51".into(),
                record_hotkey: OFF.into(),
                pin_hotkey: "f14".into(),
                action: ShotAction::Pin,
                screenshots_dir: "~/Pictures/Shots=1".into(),
                recordings_dir: "/Volumes/Big Disk/Videos".into(),
                into_meeting: false,
                delay: 5,
                draw_tool: Some(Tool::Arrow),
                draw_style: DrawStyle::parse("pen:blue:large mosaic:red:small"),
                scroll_step: ScrollStep::Large,
                scroll_max_height: 45_000,
                record_fps: 60,
                record_cursor: false,
                record_quality: Quality::High,
                record_computer_sound: false,
                record_microphone: true,
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
        // …and before dictation: voice2cursor's defaults; before
        // screenshots, their default hotkey.
        assert_eq!(old.dictation, DictationSettings::default());
        assert_eq!(old.capture, CaptureSettings::default());
        assert_eq!(
            old.capture.press_hotkey(Press::Screenshot),
            Some(crate::capture::default_hotkey())
        );
        assert_eq!(old.dictation.hotkey(), Shortcut::default());
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
        assert_eq!(s.dictation.hotkey(), Shortcut::from(Hotkey::RightAlt));
        assert_eq!(s.dictation.min_seconds, 0.35);
        assert_eq!(s.dictation.max_seconds, 300.0);
        assert!(s.dictation.config().ollama.is_some());
        assert_eq!(
            Settings::parse("dictation_hotkey=nonsense").dictation.hotkey(),
            Shortcut::default()
        );
        assert_eq!(
            Settings::parse("dictation_hotkey=ctrl+shift+vk:68")
                .dictation
                .hotkey()
                .name(),
            "ctrl+shift+vk:68"
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
    fn the_screenshot_hotkey_is_read_and_falls_back() {
        let s = Settings::parse("capture_hotkey=ctrl+shift+vk:83");
        let shot = |s: &Settings| s.capture.press_hotkey(Press::Screenshot);
        assert_eq!(shot(&s).map(|k| k.name()).as_deref(), Some("ctrl+shift+vk:83"));
        // It is not dictation's, and dictation's is not it.
        assert_eq!(s.dictation, DictationSettings::default());
        assert_eq!(
            Settings::parse("dictation_hotkey=f13").capture,
            CaptureSettings::default()
        );
        let nonsense = Settings::parse("capture_hotkey=nonsense");
        assert_eq!(shot(&nonsense), Some(crate::capture::default_hotkey()));
        assert!(nonsense.capture.press_unreadable(Press::Screenshot));
        assert!(Settings::default().to_text().contains("\ncapture_hotkey="));
        // Switched off: "off", or empty.
        for off in ["capture_hotkey=off", "capture_hotkey=", "capture_hotkey= OFF "] {
            let s = Settings::parse(off);
            assert_eq!(shot(&s), None, "{off}");
            assert!(!s.capture.press_unreadable(Press::Screenshot));
        }
    }

    #[test]
    fn the_other_capture_hotkeys_are_off_until_chosen() {
        let d = CaptureSettings::default();
        for press in [Press::Scroll, Press::Record, Press::Pin] {
            assert_eq!(d.press_hotkey(press), None, "{press:?}");
            assert_eq!(default_press_hotkey(press), None);
        }
        let s = Settings::parse("scroll_hotkey=ctrl+shift+vk:51\nrecord_hotkey=f13\npin_hotkey=nonsense").capture;
        assert_eq!(s.press_hotkey(Press::Scroll), Shortcut::parse("ctrl+shift+vk:51"));
        assert_eq!(s.press_hotkey(Press::Record), Some(Shortcut::from(Hotkey::F(13))));
        // Unreadable: its default, off.
        assert_eq!(s.press_hotkey(Press::Pin), None);
        assert!(s.press_unreadable(Press::Pin));
        // Each setting is its own.
        let mut t = CaptureSettings::default();
        for press in Press::ALL {
            *t.press_value_mut(press) = format!("f{}", 13 + press as u8);
        }
        let back = Settings::parse(
            &Settings {
                capture: t.clone(),
                ..Settings::default()
            }
            .to_text(),
        );
        for press in Press::ALL {
            assert_eq!(back.capture.press_value(press), format!("f{}", 13 + press as u8));
            assert!(back.to_text().contains(&format!("\n{}=", press_setting(press))));
        }
    }

    #[test]
    fn only_switched_on_hotkeys_are_watched_and_the_first_of_two_wins() {
        let dictation = Shortcut::default();
        let mut s = CaptureSettings::default();
        assert_eq!(s.presses(), [(Press::Screenshot, crate::capture::default_hotkey())]);
        s.hotkey = OFF.into();
        assert!(s.presses().is_empty());
        s.record_hotkey = "f13".into();
        s.pin_hotkey = "f14".into();
        assert_eq!(
            s.presses(),
            [
                (Press::Record, Shortcut::from(Hotkey::F(13))),
                (Press::Pin, Shortcut::from(Hotkey::F(14)))
            ]
        );
        assert_eq!(s.clash(Press::Record, &dictation), None);
        assert_eq!(s.clash(Press::Screenshot, &dictation), None, "off clashes with nothing");
        // The same keys twice: only the first is watched, the second is told why.
        s.pin_hotkey = "f13".into();
        assert_eq!(s.presses(), [(Press::Record, Shortcut::from(Hotkey::F(13)))]);
        assert_eq!(s.clash(Press::Pin, &dictation), Some(Clash::Press(Press::Record)));
        assert_eq!(s.clash(Press::Record, &dictation), None);
        // The dictation hotkey too.
        assert_eq!(
            s.clash(Press::Record, &Shortcut::from(Hotkey::F(13))),
            Some(Clash::Dictation)
        );
    }

    #[test]
    fn capture_choices_are_read_and_fall_back() {
        let s = Settings::parse(
            "screenshot_action=save\ncapture_delay=10\ndraw_tool=text\nscroll_step=small\n\
             scroll_max_height=100\nrecord_fps=15\nrecord_quality=low\nrecord_cursor=false\n\
             capture_into_meeting=false",
        )
        .capture;
        assert_eq!(s.action, ShotAction::Save);
        assert_eq!(s.delay, 10);
        assert_eq!(s.draw_tool, Some(Tool::Text));
        assert_eq!(s.scroll_step, ScrollStep::Small);
        assert_eq!(s.scroll_max_height, 2_000, "kept to the range");
        assert_eq!(
            (s.record_fps, s.record_quality, s.record_cursor),
            (15, Quality::Low, false)
        );
        assert!(!s.into_meeting);
        let options = s.record_options();
        assert_eq!(
            (options.fps, options.cursor, options.quality),
            (15, false, Quality::Low)
        );
        assert_eq!(s.scroll_options().step, 0.25);
        assert_eq!(s.scroll_options().max_height, 2_000);
        // Values that can't be read are the defaults.
        let bad = Settings::parse(
            "screenshot_action=print\ncapture_delay=4\ndraw_tool=crayon\nscroll_step=huge\n\
             scroll_max_height=tall\nrecord_fps=24\nrecord_quality=ultra",
        )
        .capture;
        assert_eq!(bad, CaptureSettings::default());
        assert_eq!(
            Settings::parse("scroll_max_height=999999").capture.scroll_max_height,
            60_000
        );
        // The defaults are today's.
        let d = CaptureSettings::default();
        assert_eq!(d.action, ShotAction::CopyAndSave);
        assert!(d.action.copy_saves() && d.into_meeting && d.record_cursor);
        assert!(!ShotAction::Copy.copy_saves());
        assert_eq!((d.delay, d.draw_tool, d.scroll_max_height), (0, None, 30_000));
        assert_eq!(d.scroll_step.fraction(), 0.5);
        assert_eq!((d.record_fps, d.record_quality), (30, Quality::Normal));
        assert!(Settings::default().to_text().contains("\ndraw_tool=none\n"));
        // Every choice reads back as itself.
        for a in ShotAction::ALL {
            assert_eq!(ShotAction::from_id(a.id()), Some(*a));
        }
        for t in Tool::ALL {
            assert_eq!(Tool::from_id(t.id()), Some(t));
        }
    }

    #[test]
    fn drawing_styles_are_kept_per_tool_and_bad_parts_ignored() {
        let style = DrawStyle::parse("pen:blue:large  text:white:small arrow:purple:large junk mosaic:red");
        assert_eq!(style.get(Tool::Pen), ToolStyle { color: 3, size: 2 });
        assert_eq!(style.get(Tool::Text), ToolStyle { color: 4, size: 0 });
        assert_eq!(style.get(Tool::Arrow), ToolStyle::default());
        assert_eq!(style.get(Tool::Mosaic), ToolStyle::default());
        assert_eq!(DrawStyle::parse(&style.to_text()), style);
        assert_eq!(DrawStyle::parse(""), DrawStyle::default());
        assert!(DrawStyle::check("pen:blue:large text:white:small").is_ok());
        assert!(DrawStyle::check("pen:purple:large").is_err());
        assert!(DrawStyle::check("pen:blue").is_err());
        assert!(DrawStyle::check("crayon:blue:large").is_err());
        // Out of range (never parsed, but set from the overlay): written as the default.
        let mut odd = DrawStyle::default();
        odd.set(Tool::Pen, ToolStyle { color: 99, size: 99 });
        assert!(odd.to_text().contains("pen:red:medium"));
        assert_eq!(
            DrawStyle::default().to_text(),
            "rectangle:red:medium ellipse:red:medium arrow:red:medium pen:red:medium text:red:medium mosaic:red:medium"
        );
    }

    #[test]
    fn files_go_to_the_chosen_folders_or_the_meetings_folders() {
        let meetings = Path::new("/m");
        let mut s = CaptureSettings::default();
        assert_eq!(s.loose_dir(Kind::Screenshot, meetings), Path::new("/m/Screenshots"));
        assert_eq!(s.loose_dir(Kind::Recording, meetings), Path::new("/m/Recordings"));
        s.screenshots_dir = "/shots".into();
        s.recordings_dir = "  ".into();
        assert_eq!(s.loose_dir(Kind::Screenshot, meetings), Path::new("/shots"));
        assert_eq!(s.loose_dir(Kind::Recording, meetings), Path::new("/m/Recordings"));
    }

    #[test]
    fn recordings_hear_both_until_told_otherwise() {
        let s = Settings::default();
        assert!(s.capture.record_computer_sound && s.capture.record_microphone);
        let quiet = Settings::parse("record_computer_sound=false\nrecord_microphone=false");
        assert!(!quiet.capture.sound().any());
        assert_eq!(Settings::parse(&quiet.to_text()).capture, quiet.capture);
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
