//! The hold-to-talk key: names as saved in settings, labels for the window,
//! and the macOS key codes the event tap compares against.

/// A key that starts dictation while held.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Hotkey {
    /// Right Option on the Mac; voice2cursor's default.
    #[default]
    RightAlt,
    RightCtrl,
    RightShift,
    /// Right Command on the Mac, the right Windows / Super key elsewhere.
    RightSuper,
    /// F13 to F20: keys nothing else uses.
    F(u8),
    /// Any key by its raw code on this system (a macOS virtual key code).
    Vk(u32),
}

/// Offered in the hotkey menu; any other key can be pressed instead.
pub const PRESETS: &[Hotkey] = &[
    Hotkey::RightAlt,
    Hotkey::RightSuper,
    Hotkey::RightCtrl,
    Hotkey::RightShift,
    Hotkey::F(13),
    Hotkey::F(14),
    Hotkey::F(15),
    Hotkey::F(16),
    Hotkey::F(17),
    Hotkey::F(18),
    Hotkey::F(19),
    Hotkey::F(20),
];

impl Hotkey {
    /// Reads a saved name. Also takes voice2cursor's names (`alt_r`,
    /// `cmd_r`, …), so its config can be copied over as it is.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        let key = match name.as_str() {
            "right_alt" | "right_option" | "alt_r" => Hotkey::RightAlt,
            "right_ctrl" | "right_control" | "ctrl_r" => Hotkey::RightCtrl,
            "right_shift" | "shift_r" => Hotkey::RightShift,
            "right_cmd" | "right_command" | "right_super" | "right_win" | "cmd_r" => Hotkey::RightSuper,
            _ => {
                if let Some(code) = name.strip_prefix("vk:") {
                    return code.trim().parse().ok().map(Hotkey::Vk);
                }
                let n: u8 = name.strip_prefix('f')?.parse().ok()?;
                return (13..=20).contains(&n).then_some(Hotkey::F(n));
            }
        };
        Some(key)
    }

    /// The name saved in settings.
    pub fn name(self) -> String {
        match self {
            Hotkey::RightAlt => "right_alt".into(),
            Hotkey::RightCtrl => "right_ctrl".into(),
            Hotkey::RightShift => "right_shift".into(),
            Hotkey::RightSuper => "right_cmd".into(),
            Hotkey::F(n) => format!("f{n}"),
            Hotkey::Vk(code) => format!("vk:{code}"),
        }
    }

    /// What the key is called on this system's keyboards.
    pub fn label(self) -> String {
        let mac = cfg!(target_os = "macos");
        match self {
            Hotkey::RightAlt if mac => "Right Option".into(),
            Hotkey::RightAlt => "Right Alt".into(),
            Hotkey::RightCtrl if mac => "Right Control".into(),
            Hotkey::RightCtrl => "Right Ctrl".into(),
            Hotkey::RightShift => "Right Shift".into(),
            Hotkey::RightSuper if mac => "Right Command".into(),
            Hotkey::RightSuper if cfg!(windows) => "Right Windows key".into(),
            Hotkey::RightSuper => "Right Super".into(),
            Hotkey::F(n) => format!("F{n}"),
            Hotkey::Vk(code) => format!("Key {code}"),
        }
    }
}

/// macOS virtual key codes (`kVK_*`) and the device-specific modifier bits
/// that tell the left and right keys apart in an event's flags.
#[cfg(any(target_os = "macos", test))]
pub mod mac {
    use super::Hotkey;

    const F_KEYS: [(u8, u32); 8] = [
        (13, 105),
        (14, 107),
        (15, 113),
        (16, 106),
        (17, 64),
        (18, 79),
        (19, 80),
        (20, 90),
    ];

    /// The key code `key` arrives with.
    pub fn keycode(key: Hotkey) -> u32 {
        match key {
            Hotkey::RightAlt => 61,
            Hotkey::RightCtrl => 62,
            Hotkey::RightShift => 60,
            Hotkey::RightSuper => 54,
            Hotkey::F(n) => F_KEYS.iter().find(|(f, _)| *f == n).map_or(0, |(_, code)| *code),
            Hotkey::Vk(code) => code,
        }
    }

    /// The key a pressed key code stands for, named when it has a name.
    pub fn from_keycode(code: u32) -> Hotkey {
        match code {
            61 => Hotkey::RightAlt,
            62 => Hotkey::RightCtrl,
            60 => Hotkey::RightShift,
            54 => Hotkey::RightSuper,
            _ => F_KEYS
                .iter()
                .find(|(_, c)| *c == code)
                .map_or(Hotkey::Vk(code), |(f, _)| Hotkey::F(*f)),
        }
    }

    /// For a modifier key, the flag bit set while it is held. Modifiers
    /// arrive as "flags changed" events, not key down / key up.
    pub fn modifier_bit(code: u32) -> Option<u64> {
        Some(match code {
            54 => 0x10,      // right Command
            55 => 0x08,      // left Command
            56 => 0x02,      // left Shift
            57 => 0x1_0000,  // Caps Lock
            58 => 0x20,      // left Option
            59 => 0x01,      // left Control
            60 => 0x04,      // right Shift
            61 => 0x40,      // right Option
            62 => 0x2000,    // right Control
            63 => 0x80_0000, // fn
            _ => return None,
        })
    }

    /// What an event means for the hotkey `target`: Some(true) pressed,
    /// Some(false) released, None not about it (or a key repeat).
    pub fn transition(target: u32, event: KeyEventKind, code: u32, flags: u64, repeat: bool) -> Option<bool> {
        if code != target {
            return None;
        }
        match (event, modifier_bit(code)) {
            (KeyEventKind::FlagsChanged, Some(bit)) => Some(flags & bit != 0),
            (KeyEventKind::Down, None) if !repeat => Some(true),
            (KeyEventKind::Up, None) => Some(false),
            _ => None,
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum KeyEventKind {
        Down,
        Up,
        FlagsChanged,
    }
}

#[cfg(test)]
mod tests {
    use super::mac::KeyEventKind::*;
    use super::*;

    #[test]
    fn names_aliases_and_round_trip() {
        assert_eq!(Hotkey::parse("right_alt"), Some(Hotkey::RightAlt));
        assert_eq!(Hotkey::parse("alt_r"), Some(Hotkey::RightAlt));
        assert_eq!(Hotkey::parse("cmd_r"), Some(Hotkey::RightSuper));
        assert_eq!(Hotkey::parse("right_super"), Some(Hotkey::RightSuper));
        assert_eq!(Hotkey::parse("ctrl_r"), Some(Hotkey::RightCtrl));
        assert_eq!(Hotkey::parse("shift_r"), Some(Hotkey::RightShift));
        assert_eq!(Hotkey::parse(" F13 "), Some(Hotkey::F(13)));
        assert_eq!(Hotkey::parse("f20"), Some(Hotkey::F(20)));
        assert_eq!(Hotkey::parse("vk:105"), Some(Hotkey::Vk(105)));
        assert_eq!(Hotkey::parse("f12"), None);
        assert_eq!(Hotkey::parse("f"), None);
        assert_eq!(Hotkey::parse("vk:x"), None);
        assert_eq!(Hotkey::parse("space"), None);
        for key in PRESETS.iter().copied().chain([Hotkey::Vk(7)]) {
            assert_eq!(Hotkey::parse(&key.name()), Some(key));
        }
    }

    #[test]
    fn mac_key_codes() {
        assert_eq!(mac::keycode(Hotkey::RightAlt), 61);
        assert_eq!(mac::keycode(Hotkey::F(13)), 105);
        for key in PRESETS {
            assert_eq!(mac::from_keycode(mac::keycode(*key)), *key);
        }
        assert_eq!(mac::from_keycode(0), Hotkey::Vk(0));
    }

    #[test]
    fn modifiers_and_keys_press_and_release() {
        // Right Option down, then up; left Option (58) does not count.
        assert_eq!(mac::transition(61, FlagsChanged, 61, 0x80040, false), Some(true));
        assert_eq!(mac::transition(61, FlagsChanged, 61, 0x100, false), Some(false));
        assert_eq!(mac::transition(61, FlagsChanged, 58, 0x80020, false), None);
        // Left Option held while right Option is released.
        assert_eq!(mac::transition(61, FlagsChanged, 61, 0x80020, false), Some(false));
        // F13: down, repeat ignored, up.
        assert_eq!(mac::transition(105, Down, 105, 0, false), Some(true));
        assert_eq!(mac::transition(105, Down, 105, 0, true), None);
        assert_eq!(mac::transition(105, Up, 105, 0, false), Some(false));
        assert_eq!(mac::transition(105, Down, 9, 0, false), None);
    }
}
