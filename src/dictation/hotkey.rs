//! The hold-to-talk shortcut: one key or a combination of keys (Ctrl+Shift+D,
//! Right Ctrl, F13+F14, …), names as saved in settings, labels for the
//! window, the key codes each system's watcher compares against (macOS
//! virtual key codes, Windows virtual-key codes, X11 key codes), and the
//! `Tracker` every watcher feeds its key events through.

/// A key that starts dictation while held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotkey {
    /// Right Option on the Mac; voice2cursor's default there.
    RightAlt,
    RightCtrl,
    RightShift,
    /// Right Command on the Mac, the right Windows / Super key elsewhere.
    RightSuper,
    /// F13 to F20: keys nothing else uses.
    F(u8),
    /// Any key by its raw code on this system: a macOS virtual key code, a
    /// Windows virtual-key code or an X11 key code.
    Vk(u32),
}

impl Default for Hotkey {
    /// Right Option on the Mac. Elsewhere Right Ctrl: on many keyboard
    /// layouts Right Alt is AltGr, which types characters.
    fn default() -> Self {
        if cfg!(target_os = "macos") {
            Hotkey::RightAlt
        } else {
            Hotkey::RightCtrl
        }
    }
}

/// Offered in the hotkey menu, the default first; any other key can be
/// pressed instead.
#[cfg(target_os = "macos")]
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
#[cfg(not(target_os = "macos"))]
pub const PRESETS: &[Hotkey] = &[
    Hotkey::RightCtrl,
    Hotkey::RightAlt,
    Hotkey::RightSuper,
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
            Hotkey::Vk(code) => native_name(code).unwrap_or_else(|| format!("Key {code}")),
        }
    }
}

impl Hotkey {
    /// Whether it is a modifier (Ctrl, Alt, Shift, Super, either side).
    pub fn is_modifier(self) -> bool {
        let codes = native_codes();
        !(codes.modifier)((codes.code)(self)).is_empty()
    }
}

/// This system's key codes.
pub fn native_codes() -> &'static Codes {
    #[cfg(target_os = "macos")]
    return &mac::CODES;
    #[cfg(windows)]
    return &win::CODES;
    #[cfg(not(any(target_os = "macos", windows)))]
    return &x11::CODES;
}

/// What this system's keyboards call the key with this raw code.
fn native_name(code: u32) -> Option<String> {
    #[cfg(target_os = "macos")]
    return mac::name(code);
    #[cfg(windows)]
    return win::name(code);
    #[cfg(not(any(target_os = "macos", windows)))]
    return x11::name(code);
}

/// Modifiers of a shortcut, either side: Ctrl, Alt (Option), Shift and
/// Super (Command, the Windows key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const ALT: Mods = Mods(2);
    pub const SHIFT: Mods = Mods(4);
    pub const SUPER: Mods = Mods(8);
    /// In the order they are named.
    const EACH: [Mods; 4] = [Mods::CTRL, Mods::ALT, Mods::SHIFT, Mods::SUPER];

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }

    /// The single modifiers set here, in naming order.
    pub fn each(self) -> impl Iterator<Item = Mods> {
        Mods::EACH.into_iter().filter(move |m| self.contains(*m))
    }

    fn parse(name: &str) -> Option<Mods> {
        Some(match name {
            "ctrl" | "control" => Mods::CTRL,
            "alt" | "option" | "opt" => Mods::ALT,
            "shift" => Mods::SHIFT,
            "super" | "cmd" | "command" | "win" | "meta" | "logo" => Mods::SUPER,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Mods::CTRL => "ctrl",
            Mods::ALT => "alt",
            Mods::SHIFT => "shift",
            _ => "super",
        }
    }

    fn label(self) -> &'static str {
        let mac = cfg!(target_os = "macos");
        match self {
            Mods::CTRL if mac => "Control",
            Mods::CTRL => "Ctrl",
            Mods::ALT if mac => "Option",
            Mods::ALT => "Alt",
            Mods::SHIFT => "Shift",
            _ if mac => "Command",
            _ if cfg!(windows) => "Win",
            _ => "Super",
        }
    }
}

/// What starts dictation: every key in `keys` held, together with the
/// modifiers in `mods` (either side of each). One plain key has no `mods`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub mods: Mods,
    /// Never empty.
    pub keys: Vec<Hotkey>,
}

impl From<Hotkey> for Shortcut {
    fn from(key: Hotkey) -> Self {
        Shortcut {
            mods: Mods::NONE,
            keys: vec![key],
        }
    }
}

impl Default for Shortcut {
    fn default() -> Self {
        Hotkey::default().into()
    }
}

impl Shortcut {
    /// Reads a saved name: one key's name (`right_ctrl`, `f13`, `vk:65`), or
    /// several joined with `+`, modifiers by their names
    /// (`ctrl+shift+vk:68`).
    pub fn parse(name: &str) -> Option<Self> {
        let mut mods = Mods::NONE;
        let mut keys = Vec::new();
        for part in name.split('+') {
            let part = part.trim().to_ascii_lowercase();
            if let Some(m) = Mods::parse(&part) {
                mods = mods.union(m);
            } else {
                let key = Hotkey::parse(&part)?;
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        (!keys.is_empty()).then_some(Shortcut { mods, keys })
    }

    /// The name saved in settings.
    pub fn name(&self) -> String {
        let mods = self.mods.each().map(|m| m.name().to_string());
        let keys = self.keys.iter().map(|k| k.name());
        mods.chain(keys).collect::<Vec<_>>().join("+")
    }

    /// What the window calls it, e.g. "Ctrl+Shift+D".
    pub fn label(&self) -> String {
        let mods = self.mods.each().map(|m| m.label().to_string());
        let keys = self.keys.iter().map(|k| k.label());
        mods.chain(keys).collect::<Vec<_>>().join("+")
    }

    /// The one key, when it is a single key with no modifiers.
    pub fn single(&self) -> Option<Hotkey> {
        match self.keys.as_slice() {
            [key] if self.mods.is_empty() => Some(*key),
            _ => None,
        }
    }

    /// Whether it is held right now, with `is_down` saying whether a key
    /// code is down.
    pub fn held_now(&self, codes: &Codes, is_down: impl Fn(u32) -> bool) -> bool {
        self.keys.iter().all(|k| is_down((codes.code)(*k)))
            && self
                .mods
                .each()
                .all(|m| codes.modifiers.iter().any(|c| (codes.modifier)(*c) == m && is_down(*c)))
    }

    /// The shortcut the keys pressed (in this order) make: modifiers count
    /// either side once another key is among them; keys made of modifiers
    /// alone keep the last one pressed as it is (Right Ctrl, say).
    fn from_pressed(codes: &Codes, pressed: &[u32]) -> Shortcut {
        let is_mod = |c: &u32| !(codes.modifier)(*c).is_empty();
        let plain: Vec<u32> = pressed.iter().copied().filter(|c| !is_mod(c)).collect();
        let (mod_codes, key_codes): (Vec<u32>, Vec<u32>) = if plain.is_empty() {
            let (last, rest) = pressed.split_last().expect("a key was pressed");
            (rest.to_vec(), vec![*last])
        } else {
            (pressed.iter().copied().filter(is_mod).collect(), plain)
        };
        Shortcut {
            mods: mod_codes.iter().fold(Mods::NONE, |m, c| m.union((codes.modifier)(*c))),
            keys: key_codes.into_iter().map(codes.from_code).collect(),
        }
    }
}

/// One system's key codes, for the `Tracker`.
pub struct Codes {
    /// The code `key` arrives with.
    pub code: fn(Hotkey) -> u32,
    /// The key a code stands for, named when it has a name.
    pub from_code: fn(u32) -> Hotkey,
    /// The modifier a code is, or `NONE`.
    pub modifier: fn(u32) -> Mods,
    /// Every modifier key's code.
    pub modifiers: &'static [u32],
    pub escape: u32,
}

/// What a key event means, as the `Tracker` sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The shortcut is now held…
    Down,
    /// …and no longer.
    Up,
    /// The keys pressed while capturing, once the first was let go of.
    Captured(Shortcut),
    /// Escape, on its own, while capturing.
    CaptureCancelled,
}

/// Follows which keys are down, from each key going down and up, to tell
/// when the shortcut is held and to capture a new one. Repeats (a key
/// already down going down again) change nothing.
#[derive(Debug, Default)]
pub struct Tracker {
    down: Vec<u32>,
    active: bool,
    capturing: bool,
    /// The keys pressed since capturing began, in order.
    pressed: Vec<u32>,
}

impl Tracker {
    /// Whether the shortcut is held, as last reported.
    pub fn active(&self) -> bool {
        self.active
    }

    /// The modifiers held other than by keys in `except`.
    pub fn held_mods(&self, codes: &Codes, except: &[u32]) -> Mods {
        self.down
            .iter()
            .filter(|c| !except.contains(c))
            .fold(Mods::NONE, |m, c| m.union((codes.modifier)(*c)))
    }

    fn holds(&self, codes: &Codes, target: &Shortcut) -> bool {
        let keys: Vec<u32> = target.keys.iter().map(|k| (codes.code)(*k)).collect();
        keys.iter().all(|c| self.down.contains(c)) && self.held_mods(codes, &keys).contains(target.mods)
    }

    /// Takes in a key going down or up. `capturing`: whether the keys are
    /// being captured as the new shortcut rather than compared with
    /// `target`.
    pub fn key(&mut self, codes: &Codes, target: &Shortcut, capturing: bool, code: u32, down: bool) -> Option<Outcome> {
        let repeat = down && self.down.contains(&code);
        if !down {
            self.down.retain(|c| *c != code);
        } else if !repeat {
            self.down.push(code);
        }
        if capturing != self.capturing {
            self.capturing = capturing;
            self.pressed.clear();
        }
        if capturing {
            if down {
                if !self.pressed.contains(&code) {
                    if code == codes.escape && self.pressed.is_empty() {
                        return Some(Outcome::CaptureCancelled);
                    }
                    self.pressed.push(code);
                }
                return None;
            }
            // A key held since before capturing began is not part of it.
            if !self.pressed.contains(&code) {
                return None;
            }
            let pressed = std::mem::take(&mut self.pressed);
            return Some(Outcome::Captured(Shortcut::from_pressed(codes, &pressed)));
        }
        let held = self.holds(codes, target);
        if held == self.active {
            return None;
        }
        self.active = held;
        Some(if held { Outcome::Down } else { Outcome::Up })
    }
}

/// macOS virtual key codes (`kVK_*`) and the device-specific modifier bits
/// that tell the left and right keys apart in an event's flags.
#[cfg(any(target_os = "macos", test))]
pub mod mac {
    use super::{Codes, Hotkey, Mods};

    pub const ESCAPE: u32 = 53;

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

    pub fn modifier(code: u32) -> Mods {
        match code {
            54 | 55 => Mods::SUPER,
            56 | 60 => Mods::SHIFT,
            58 | 61 => Mods::ALT,
            59 | 62 => Mods::CTRL,
            _ => Mods::NONE,
        }
    }

    pub const CODES: Codes = Codes {
        code: keycode,
        from_code: from_keycode,
        modifier,
        modifiers: &[54, 55, 56, 58, 59, 60, 61, 62],
        escape: ESCAPE,
    };

    /// What US keyboards print on the key with this code.
    pub fn name(code: u32) -> Option<String> {
        const LETTERS: [(u32, &str); 36] = [
            (0, "A"),
            (11, "B"),
            (8, "C"),
            (2, "D"),
            (14, "E"),
            (3, "F"),
            (5, "G"),
            (4, "H"),
            (34, "I"),
            (38, "J"),
            (40, "K"),
            (37, "L"),
            (46, "M"),
            (45, "N"),
            (31, "O"),
            (35, "P"),
            (12, "Q"),
            (15, "R"),
            (1, "S"),
            (17, "T"),
            (32, "U"),
            (9, "V"),
            (13, "W"),
            (7, "X"),
            (16, "Y"),
            (6, "Z"),
            (29, "0"),
            (18, "1"),
            (19, "2"),
            (20, "3"),
            (21, "4"),
            (23, "5"),
            (22, "6"),
            (26, "7"),
            (28, "8"),
            (25, "9"),
        ];
        const F1_TO_F12: [u32; 12] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];
        if let Some((_, n)) = LETTERS.iter().find(|(c, _)| *c == code) {
            return Some((*n).into());
        }
        if let Some(i) = F1_TO_F12.iter().position(|c| *c == code) {
            return Some(format!("F{}", i + 1));
        }
        Some(
            match code {
                49 => "Space",
                36 => "Return",
                48 => "Tab",
                56 => "Left Shift",
                59 => "Left Control",
                58 => "Left Option",
                55 => "Left Command",
                63 => "fn",
                _ => return None,
            }
            .into(),
        )
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

    /// Whether an event puts its key down (true) or up; None when it is
    /// not about a key going down or up.
    pub fn key_change(event: KeyEventKind, code: u32, flags: u64) -> Option<bool> {
        match (event, modifier_bit(code)) {
            (KeyEventKind::FlagsChanged, Some(bit)) => Some(flags & bit != 0),
            (KeyEventKind::Down, None) => Some(true),
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

/// The F13 to F20 keys' codes from F13 up, starting at `first`.
#[cfg(any(not(target_os = "macos"), test))]
fn f_key(code: u32, first: u32) -> Option<Hotkey> {
    let n = code.checked_sub(first)?;
    (n < 8).then(|| Hotkey::F(13 + n as u8))
}

/// Windows virtual-key codes (`VK_*`), as the low-level keyboard hook sees
/// them: it tells the left and right modifiers apart.
#[cfg(any(windows, test))]
pub mod win {
    use super::{Codes, Hotkey, Mods, f_key};

    pub const ESCAPE: u32 = 0x1B;
    const RSHIFT: u32 = 0xA1;
    const RCONTROL: u32 = 0xA3;
    const RMENU: u32 = 0xA5;
    const RWIN: u32 = 0x5C;
    const F13: u32 = 0x7C;

    /// The virtual-key code `key` arrives with.
    pub fn vk(key: Hotkey) -> u32 {
        match key {
            Hotkey::RightAlt => RMENU,
            Hotkey::RightCtrl => RCONTROL,
            Hotkey::RightShift => RSHIFT,
            Hotkey::RightSuper => RWIN,
            Hotkey::F(n) => F13 + u32::from(n.clamp(13, 20) - 13),
            Hotkey::Vk(code) => code,
        }
    }

    /// The key a pressed virtual-key code stands for, named when it has a name.
    pub fn from_vk(code: u32) -> Hotkey {
        match code {
            RMENU => Hotkey::RightAlt,
            RCONTROL => Hotkey::RightCtrl,
            RSHIFT => Hotkey::RightShift,
            RWIN => Hotkey::RightSuper,
            _ => f_key(code, F13).unwrap_or(Hotkey::Vk(code)),
        }
    }

    pub fn modifier(code: u32) -> Mods {
        match code {
            0x10 | 0xA0 | RSHIFT => Mods::SHIFT,
            0x11 | 0xA2 | RCONTROL => Mods::CTRL,
            0x12 | 0xA4 | RMENU => Mods::ALT,
            0x5B | RWIN => Mods::SUPER,
            _ => Mods::NONE,
        }
    }

    pub const CODES: Codes = Codes {
        code: vk,
        from_code: from_vk,
        modifier,
        modifiers: &[0xA0, RSHIFT, 0xA2, RCONTROL, 0xA4, RMENU, 0x5B, RWIN],
        escape: ESCAPE,
    };

    /// What the key with this virtual-key code is called.
    pub fn name(code: u32) -> Option<String> {
        if let Some(c) = char::from_u32(code).filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Some(c.to_string());
        }
        if (0x70..=0x7B).contains(&code) {
            return Some(format!("F{}", code - 0x6F));
        }
        Some(
            match code {
                0x20 => "Space",
                0x0D => "Enter",
                0x09 => "Tab",
                0x08 => "Backspace",
                0x2D => "Insert",
                0x2E => "Delete",
                0x24 => "Home",
                0x23 => "End",
                0x21 => "Page Up",
                0x22 => "Page Down",
                0x13 => "Pause",
                0x91 => "Scroll Lock",
                0x2C => "Print Screen",
                0xA0 => "Left Shift",
                0xA2 => "Left Ctrl",
                0xA4 => "Left Alt",
                0x5B => "Left Windows key",
                0x5D => "Menu key",
                _ => return None,
            }
            .into(),
        )
    }
}

/// X11 key codes, as XInput 2 raw events carry them: the Linux evdev codes
/// plus 8, the same on every layout of a PC keyboard.
#[cfg(any(not(any(target_os = "macos", windows)), test))]
pub mod x11 {
    use super::{Codes, Hotkey, Mods, f_key};

    pub const ESCAPE: u32 = 9;
    const RIGHT_SHIFT: u32 = 62;
    const RIGHT_CTRL: u32 = 105;
    const RIGHT_ALT: u32 = 108;
    const RIGHT_SUPER: u32 = 134;
    const F13: u32 = 191;

    /// The key code `key` arrives with.
    pub fn keycode(key: Hotkey) -> u32 {
        match key {
            Hotkey::RightAlt => RIGHT_ALT,
            Hotkey::RightCtrl => RIGHT_CTRL,
            Hotkey::RightShift => RIGHT_SHIFT,
            Hotkey::RightSuper => RIGHT_SUPER,
            Hotkey::F(n) => F13 + u32::from(n.clamp(13, 20) - 13),
            Hotkey::Vk(code) => code,
        }
    }

    /// The key a pressed key code stands for, named when it has a name.
    pub fn from_keycode(code: u32) -> Hotkey {
        match code {
            RIGHT_ALT => Hotkey::RightAlt,
            RIGHT_CTRL => Hotkey::RightCtrl,
            RIGHT_SHIFT => Hotkey::RightShift,
            RIGHT_SUPER => Hotkey::RightSuper,
            _ => f_key(code, F13).unwrap_or(Hotkey::Vk(code)),
        }
    }

    pub fn modifier(code: u32) -> Mods {
        match code {
            50 | RIGHT_SHIFT => Mods::SHIFT,
            37 | RIGHT_CTRL => Mods::CTRL,
            64 | RIGHT_ALT => Mods::ALT,
            133 | RIGHT_SUPER => Mods::SUPER,
            _ => Mods::NONE,
        }
    }

    pub const CODES: Codes = Codes {
        code: keycode,
        from_code: from_keycode,
        modifier,
        modifiers: &[50, RIGHT_SHIFT, 37, RIGHT_CTRL, 64, RIGHT_ALT, 133, RIGHT_SUPER],
        escape: ESCAPE,
    };

    /// What a PC keyboard prints on the key with this code.
    pub fn name(code: u32) -> Option<String> {
        const ROWS: [(u32, &str); 4] = [
            (10, "1234567890"),
            (24, "QWERTYUIOP"),
            (38, "ASDFGHJKL"),
            (52, "ZXCVBNM"),
        ];
        for (first, keys) in ROWS {
            if let Some(c) = code.checked_sub(first).and_then(|i| keys.chars().nth(i as usize)) {
                return Some(c.to_string());
            }
        }
        if (67..=76).contains(&code) {
            return Some(format!("F{}", code - 66));
        }
        Some(
            match code {
                95 => "F11",
                96 => "F12",
                65 => "Space",
                36 => "Enter",
                23 => "Tab",
                22 => "Backspace",
                118 => "Insert",
                119 => "Delete",
                110 => "Home",
                115 => "End",
                112 => "Page Up",
                117 => "Page Down",
                127 => "Pause",
                78 => "Scroll Lock",
                107 => "Print Screen",
                50 => "Left Shift",
                37 => "Left Ctrl",
                64 => "Left Alt",
                133 => "Left Super",
                135 => "Menu key",
                _ => return None,
            }
            .into(),
        )
    }

    /// Whether `code` is down in a keymap as `QueryKeymap` returns it: one
    /// bit per key code, from code 0.
    pub fn is_down(keymap: &[u8], code: u32) -> bool {
        keymap
            .get(code as usize / 8)
            .is_some_and(|byte| byte & (1 << (code % 8)) != 0)
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
    fn default_is_offered_first() {
        assert_eq!(PRESETS[0], Hotkey::default());
        let expected = if cfg!(target_os = "macos") {
            Hotkey::RightAlt
        } else {
            Hotkey::RightCtrl
        };
        assert_eq!(Hotkey::default(), expected);
    }

    #[test]
    fn windows_key_codes() {
        assert_eq!(win::vk(Hotkey::RightCtrl), 0xA3);
        assert_eq!(win::vk(Hotkey::RightAlt), 0xA5);
        assert_eq!(win::vk(Hotkey::RightSuper), 0x5C);
        assert_eq!(win::vk(Hotkey::F(13)), 0x7C);
        assert_eq!(win::vk(Hotkey::F(20)), 0x83);
        for key in PRESETS.iter().copied().chain([Hotkey::Vk(0x41)]) {
            assert_eq!(win::from_vk(win::vk(key)), key);
        }
        // F12 and F21 are not hotkeys of their own.
        assert_eq!(win::from_vk(0x7B), Hotkey::Vk(0x7B));
        assert_eq!(win::from_vk(0x84), Hotkey::Vk(0x84));
    }

    /// Feeds `(code, down)` events in, keeping what came out.
    fn run(target: &Shortcut, capturing: bool, events: &[(u32, bool)]) -> Vec<Outcome> {
        let mut tracker = Tracker::default();
        events
            .iter()
            .filter_map(|(code, down)| tracker.key(&win::CODES, target, capturing, *code, *down))
            .collect()
    }

    #[test]
    fn single_keys_press_and_release_and_repeats_are_not_presses() {
        let target = Shortcut::from(Hotkey::RightCtrl);
        // Held: Windows sends key-down again and again. Left Ctrl is not it.
        let events = [
            (0xA3, true),
            (0xA3, true),
            (0xA2, true),
            (0xA2, false),
            (0xA3, false),
            (0xA3, true),
        ];
        assert_eq!(
            run(&target, false, &events),
            [Outcome::Down, Outcome::Up, Outcome::Down]
        );
    }

    #[test]
    fn combinations_need_every_key_and_either_side_of_a_modifier() {
        // Ctrl+Shift+D.
        let target = Shortcut::parse("ctrl+shift+vk:68").unwrap();
        // D alone, then with left Ctrl only: nothing.
        assert_eq!(
            run(
                &target,
                false,
                &[(0x44, true), (0x44, false), (0xA2, true), (0x44, true)]
            ),
            []
        );
        // Right Ctrl, left Shift, D: down; repeats of D change nothing;
        // letting go of Shift ends it.
        let events = [
            (0xA3, true),
            (0xA0, true),
            (0x44, true),
            (0x44, true),
            (0xA0, false),
            (0x44, false),
        ];
        assert_eq!(run(&target, false, &events), [Outcome::Down, Outcome::Up]);
        // The keys in any order: D first, the modifiers after.
        let events = [(0x44, true), (0xA2, true), (0xA1, true), (0x44, false)];
        assert_eq!(run(&target, false, &events), [Outcome::Down, Outcome::Up]);
        // Two plain keys together.
        let both = Shortcut::parse("f13+f14").unwrap();
        let events = [(0x7C, true), (0x7D, true), (0x7C, false)];
        assert_eq!(run(&both, false, &events), [Outcome::Down, Outcome::Up]);
    }

    #[test]
    fn capture_takes_every_key_held_until_the_first_is_let_go() {
        let any = Shortcut::default();
        // Ctrl, Shift and D held, then D let go of: Ctrl+Shift+D.
        let events = [(0xA2, true), (0xA1, true), (0x44, true), (0x44, true), (0x44, false)];
        assert_eq!(
            run(&any, true, &events),
            [Outcome::Captured(Shortcut::parse("ctrl+shift+vk:68").unwrap())]
        );
        // A modifier alone keeps its side.
        assert_eq!(
            run(&any, true, &[(0xA3, true), (0xA3, true), (0xA3, false)]),
            [Outcome::Captured(Shortcut::from(Hotkey::RightCtrl))]
        );
        // Modifiers alone: the last one pressed is the key.
        assert_eq!(
            run(&any, true, &[(0xA2, true), (0xA5, true), (0xA2, false)]),
            [Outcome::Captured(Shortcut {
                mods: Mods::CTRL,
                keys: vec![Hotkey::RightAlt]
            })]
        );
        // Two plain keys.
        assert_eq!(
            run(&any, true, &[(0x7C, true), (0x7D, true), (0x7D, false)]),
            [Outcome::Captured(Shortcut::parse("f13+f14").unwrap())]
        );
        // Escape on its own cancels; with other keys it is one of them.
        assert_eq!(run(&any, true, &[(0x1B, true)]), [Outcome::CaptureCancelled]);
        assert_eq!(
            run(&any, true, &[(0xA2, true), (0x1B, true), (0x1B, false)]),
            [Outcome::Captured(Shortcut {
                mods: Mods::CTRL,
                keys: vec![Hotkey::Vk(0x1B)]
            })]
        );
        // A key held from before capturing began is not part of it.
        let mut tracker = Tracker::default();
        assert_eq!(tracker.key(&win::CODES, &any, false, 0x41, true), None);
        assert_eq!(tracker.key(&win::CODES, &any, true, 0x41, false), None);
        assert_eq!(
            tracker
                .key(&win::CODES, &any, true, 0x42, true)
                .or(tracker.key(&win::CODES, &any, true, 0x42, false)),
            Some(Outcome::Captured(Shortcut::from(Hotkey::Vk(0x42))))
        );
    }

    #[test]
    fn shortcut_names_and_labels() {
        let s = Shortcut::parse(" Ctrl + Shift + vk:68 ").unwrap();
        assert_eq!(s.mods, Mods::CTRL.union(Mods::SHIFT));
        assert_eq!(s.keys, [Hotkey::Vk(68)]);
        assert_eq!(s.name(), "ctrl+shift+vk:68");
        assert_eq!(Shortcut::parse(&s.name()), Some(s.clone()));
        assert_eq!(s.single(), None);
        // Old single-key names read as before.
        assert_eq!(Shortcut::parse("alt_r"), Some(Shortcut::from(Hotkey::RightAlt)));
        assert_eq!(Shortcut::parse("right_ctrl").unwrap().single(), Some(Hotkey::RightCtrl));
        assert_eq!(Shortcut::from(Hotkey::F(13)).name(), "f13");
        assert_eq!(Shortcut::parse("cmd+option+f13").unwrap().name(), "alt+super+f13");
        // Modifiers alone, or a key that isn't one, are not shortcuts.
        assert_eq!(Shortcut::parse("ctrl+shift"), None);
        assert_eq!(Shortcut::parse("ctrl+nonsense"), None);
        assert_eq!(Shortcut::parse(""), None);
        assert_eq!(Shortcut::parse("f13+f13"), Some(Shortcut::from(Hotkey::F(13))));
        assert!(Shortcut::parse("ctrl+f13").unwrap().label().ends_with("+F13"));
        assert_eq!(win::name(0x44).as_deref(), Some("D"));
        assert_eq!(win::name(0x7B).as_deref(), Some("F12"));
        assert_eq!(x11::name(40).as_deref(), Some("D"));
        assert_eq!(x11::name(19).as_deref(), Some("0"));
        assert_eq!(x11::name(58).as_deref(), Some("M"));
        assert_eq!(mac::name(2).as_deref(), Some("D"));
        assert_eq!(mac::name(122).as_deref(), Some("F1"));
    }

    #[test]
    fn held_now_asks_for_every_key_and_either_side() {
        let s = Shortcut::parse("ctrl+vk:68").unwrap();
        assert!(s.held_now(&win::CODES, |c| c == 0x44 || c == 0xA3));
        assert!(s.held_now(&win::CODES, |c| c == 0x44 || c == 0xA2));
        assert!(!s.held_now(&win::CODES, |c| c == 0x44));
        assert!(!s.held_now(&win::CODES, |c| c == 0xA2));
    }

    #[test]
    fn x11_key_codes() {
        assert_eq!(x11::keycode(Hotkey::RightCtrl), 105);
        assert_eq!(x11::keycode(Hotkey::RightAlt), 108);
        assert_eq!(x11::keycode(Hotkey::F(13)), 191);
        for key in PRESETS.iter().copied().chain([Hotkey::Vk(38)]) {
            assert_eq!(x11::from_keycode(x11::keycode(key)), key);
        }
        assert_eq!(x11::from_keycode(199), Hotkey::Vk(199));
        let mut keymap = [0u8; 32];
        keymap[105 / 8] |= 1 << (105 % 8);
        assert!(x11::is_down(&keymap, 105));
        assert!(!x11::is_down(&keymap, 104));
        assert!(!x11::is_down(&keymap, 300));
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
    fn mac_modifiers_and_keys_go_down_and_up() {
        // Right Option down, then up, from the flags; left Option is 58.
        assert_eq!(mac::key_change(FlagsChanged, 61, 0x80040), Some(true));
        assert_eq!(mac::key_change(FlagsChanged, 61, 0x100), Some(false));
        assert_eq!(mac::key_change(FlagsChanged, 61, 0x80020), Some(false));
        assert_eq!(mac::key_change(FlagsChanged, 58, 0x80020), Some(true));
        assert_eq!(mac::key_change(Down, 105, 0), Some(true));
        assert_eq!(mac::key_change(Up, 105, 0), Some(false));
        assert_eq!(mac::key_change(Down, 61, 0), None);
        assert_eq!(mac::modifier(55), Mods::SUPER);
        assert_eq!(mac::modifier(2), Mods::NONE);
    }
}
