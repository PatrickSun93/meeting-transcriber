//! The plain logic of the Windows and Linux dictation backends: the tray
//! menu's items, the tray icon's pixels, the login entries, the `dictate`
//! socket's commands and the Wayland portal's names. Here, apart from the
//! system calls, so their tests run on every system.

use std::path::Path;

use super::{TrayAction, TrayIcon, TrayMenu};

// ---- The tray menu ----

/// Longest a dictation is shown in the tray menu, in characters.
const MENU_TEXT: usize = 48;
pub const ID_STATUS: u32 = 1;
const ID_HEADER: u32 = 2;
const ID_SEPARATORS: [u32; 3] = [3, 4, 5];
/// The recent dictations are `RECENT + i`.
const RECENT: u32 = 100;
pub const ID_PAUSE: u32 = 1000;
pub const ID_OPEN: u32 = 1001;
pub const ID_QUIT: u32 = 1002;

/// One line of the tray menu. Ids start at 1: Windows reports 0 for "no
/// choice", and the D-Bus menu's root is 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: u32,
    pub label: String,
    pub kind: ItemKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// A greyed-out line.
    Text,
    Action,
    /// An action with a check mark, on or off.
    Check(bool),
    Separator,
}

/// The tray menu as the Mac's menu bar item shows it: the state, the latest
/// dictations (click to copy), Pause, Open and Quit.
pub fn menu_items(menu: &TrayMenu) -> Vec<MenuItem> {
    let item = |id, label: &str, kind| MenuItem {
        id,
        label: label.to_string(),
        kind,
    };
    let separator = |i: usize| item(ID_SEPARATORS[i], "", ItemKind::Separator);
    let mut items = vec![item(ID_STATUS, &menu.status, ItemKind::Text), separator(0)];
    if menu.recent.is_empty() {
        items.push(item(ID_HEADER, "No dictations yet", ItemKind::Text));
    } else {
        items.push(item(ID_HEADER, "Recent (click to copy)", ItemKind::Text));
        for (i, (time, text)) in menu.recent.iter().enumerate() {
            items.push(item(
                RECENT + i as u32,
                &format!("{time}  {}", shorten(text)),
                ItemKind::Action,
            ));
        }
    }
    items.push(separator(1));
    items.push(item(ID_PAUSE, "Pause dictation", ItemKind::Check(menu.paused)));
    items.push(item(ID_OPEN, "Open Meeting Transcriber", ItemKind::Action));
    items.push(separator(2));
    items.push(item(ID_QUIT, "Quit Meeting Transcriber", ItemKind::Action));
    items
}

/// What choosing the item `id` of `menu` asks for.
pub fn menu_action(id: u32, menu: &TrayMenu) -> Option<TrayAction> {
    match id {
        ID_PAUSE => Some(TrayAction::TogglePause),
        ID_OPEN => Some(TrayAction::OpenWindow),
        ID_QUIT => Some(TrayAction::Quit),
        _ => {
            let i = id.checked_sub(RECENT)? as usize;
            menu.recent.get(i).map(|(_, text)| TrayAction::Copy(text.clone()))
        }
    }
}

/// A Windows menu item's text: `&` marks a shortcut letter there, so a
/// literal one is doubled, and a tab would start a second column.
#[cfg(any(windows, test))]
pub fn windows_menu_text(label: &str) -> String {
    label.replace('&', "&&").replace('\t', " ")
}

/// One line, at most `MENU_TEXT` characters.
fn shorten(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(MENU_TEXT) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

// ---- The tray icon ----

/// A filled circle in the state's colour with a white microphone on it
/// (crossed out when paused, `!` after an error), `size` pixels square.
/// Rows from the top, each pixel RGBA with straight alpha.
pub fn icon_rgba(icon: TrayIcon, size: u32) -> Vec<[u8; 4]> {
    let color: [f32; 3] = match icon {
        TrayIcon::Loading => [112.0, 122.0, 138.0],
        TrayIcon::Ready => [46.0, 160.0, 67.0],
        TrayIcon::Recording => [220.0, 38.0, 38.0],
        TrayIcon::Transcribing => [217.0, 119.0, 6.0],
        TrayIcon::Paused => [110.0, 110.0, 110.0],
        TrayIcon::Error => [190.0, 24.0, 93.0],
    };
    // Shapes are drawn on a 32-unit square; `scale` is pixels per unit.
    let scale = size as f32 / 32.0;
    let cover = |distance: f32| (0.5 - distance * scale).clamp(0.0, 1.0);
    let mut pixels = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
            let disk = cover(length(p, (16.0, 16.0)) - 15.0);
            let glyph = match icon {
                TrayIcon::Error => cover(exclamation(p)),
                TrayIcon::Paused => {
                    let slash = segment(p, (8.0, 24.0), (24.0, 8.0));
                    let gap = cover(slash - 3.0);
                    (cover(microphone(p)) * (1.0 - gap)).max(cover(slash - 1.5))
                }
                _ => cover(microphone(p)),
            };
            let channel = |c: f32| (c + (255.0 - c) * glyph).round() as u8;
            pixels.push([
                channel(color[0]),
                channel(color[1]),
                channel(color[2]),
                (disk * 255.0).round() as u8,
            ]);
        }
    }
    pixels
}

fn length(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// How far `p` is from the line segment `a`–`b`.
fn segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ab, ap) = ((b.0 - a.0, b.1 - a.1), (p.0 - a.0, p.1 - a.1));
    let t = ((ap.0 * ab.0 + ap.1 * ab.1) / (ab.0 * ab.0 + ab.1 * ab.1)).clamp(0.0, 1.0);
    length(p, (a.0 + ab.0 * t, a.1 + ab.1 * t))
}

/// Signed distance to a microphone: the capsule, its cradle, the stand.
fn microphone(p: (f32, f32)) -> f32 {
    let capsule = segment(p, (16.0, 9.0), (16.0, 15.0)) - 4.5;
    let center = (16.0, 15.5);
    let cradle = if p.1 >= center.1 {
        (length(p, center) - 7.5).abs() - 1.25
    } else {
        length(p, (8.5, 15.5)).min(length(p, (23.5, 15.5))) - 1.25
    };
    let stem = segment(p, (16.0, 23.0), (16.0, 26.0)) - 1.25;
    let base = segment(p, (12.0, 26.0), (20.0, 26.0)) - 1.25;
    capsule.min(cradle).min(stem).min(base)
}

fn exclamation(p: (f32, f32)) -> f32 {
    let bar = segment(p, (16.0, 8.0), (16.0, 18.5)) - 2.2;
    let dot = length(p, (16.0, 24.0)) - 2.4;
    bar.min(dot)
}

// ---- Opening at login ----

/// The command the Windows `Run` key starts at login: the app, hidden in
/// the notification area.
#[cfg(any(windows, test))]
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" --hidden", exe.display())
}

/// `~/.config/autostart/meeting-transcriber.desktop`: the desktop starts
/// the app at login, hidden in the tray.
#[cfg(any(not(any(target_os = "macos", windows)), test))]
pub fn autostart_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Meeting Transcriber\n\
         Comment=Live meeting transcription and dictation\nExec={} --hidden\n\
         Icon=meeting-transcriber\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        desktop_exec_arg(&exe.to_string_lossy())
    )
}

/// One argument of a desktop entry's `Exec`, quoted. The spec's quoting
/// escapes `"`, `` ` ``, `$` and `\`; the string escaping of all values
/// then doubles every backslash, and `%` starts a field code.
#[cfg(any(not(any(target_os = "macos", windows)), test))]
fn desktop_exec_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '\\' => out.push_str("\\\\\\\\"),
            '%' => out.push_str("%%"),
            '\n' | '\r' => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---- `meeting-transcriber dictate` ----

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// What `meeting-transcriber dictate …` asks the running app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictateCommand {
    /// As if the hotkey went down…
    Start,
    /// …and came up.
    Stop,
    /// Start, or stop if started: for desktops whose shortcuts only fire
    /// on press.
    Toggle,
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
impl DictateCommand {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "start" | "down" => Some(Self::Start),
            "stop" | "up" => Some(Self::Stop),
            "toggle" => Some(Self::Toggle),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Toggle => "toggle",
        }
    }

    /// The hotkey event it stands for. A toggle is decided by the
    /// recorder, which knows whether it records.
    pub fn key_event(self) -> super::KeyEvent {
        match self {
            Self::Start => super::KeyEvent::Down,
            Self::Stop => super::KeyEvent::Up,
            Self::Toggle => super::KeyEvent::Toggle,
        }
    }

    /// The running app's answer, after `ok `.
    pub fn done(self) -> &'static str {
        match self {
            Self::Start => "started",
            Self::Stop => "stopped",
            Self::Toggle => "toggled",
        }
    }
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// Where the running app listens for `dictate`: in `XDG_RUNTIME_DIR`,
/// which only this user can open, else in the app data folder.
pub fn socket_path(runtime_dir: Option<&Path>, data_dir: &Path) -> std::path::PathBuf {
    match runtime_dir.filter(|d| d.is_absolute()) {
        Some(dir) => dir.join("meeting-transcriber.sock"),
        None => data_dir.join("dictate.sock"),
    }
}

// ---- Linux: where the hotkey comes from, how text is pasted ----

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// A way to see the hotkey on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// XInput 2 raw key events: the key itself, held and let go.
    X11,
    /// The desktop's global shortcuts portal (Wayland); the desktop picks
    /// the key.
    Portal,
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// The ways to try, in order. Under Wayland, X11 only sees keys typed into
/// X11 apps, so it is not used there.
pub fn key_sources(wayland: bool, x11: bool) -> Vec<KeySource> {
    match (wayland, x11) {
        (true, _) => vec![KeySource::Portal],
        (false, true) => vec![KeySource::X11, KeySource::Portal],
        (false, false) => vec![KeySource::Portal],
    }
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// A way to press Ctrl+V on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteTool {
    XTest,
    Wtype,
    Ydotool,
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// The ways to try, in order; none means the text is only copied. GNOME
/// does not let wtype type, so ydotool comes first there.
pub fn paste_tools(wayland: bool, gnome: bool, installed: impl Fn(&str) -> bool) -> Vec<PasteTool> {
    if !wayland {
        return vec![PasteTool::XTest];
    }
    let order = if gnome {
        [PasteTool::Ydotool, PasteTool::Wtype]
    } else {
        [PasteTool::Wtype, PasteTool::Ydotool]
    };
    order
        .into_iter()
        .filter(|tool| {
            installed(match tool {
                PasteTool::Wtype => "wtype",
                _ => "ydotool",
            })
        })
        .collect()
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// The key code whose first keysym is `keysym`, in a keyboard mapping as
/// `GetKeyboardMapping` returns it: `per` keysyms for each key code from
/// `first`.
pub fn keycode_for(keysym: u32, first: u8, per: u8, keysyms: &[u32]) -> Option<u8> {
    let per = usize::from(per.max(1));
    keysyms
        .chunks(per)
        .position(|codes| codes.first() == Some(&keysym))
        .and_then(|i| u8::try_from(usize::from(first) + i).ok())
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// The object a portal request answers on: `…/request/SENDER/TOKEN`, with
/// the caller's unique bus name minus its `:` and with `_` for `.`.
pub fn portal_request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// The key suggested to the portal for the shortcut. Shortcuts there are
/// key combinations, so a lone modifier has nothing to suggest.
pub fn portal_trigger(key: crate::dictation::hotkey::Hotkey) -> Option<String> {
    match key {
        crate::dictation::hotkey::Hotkey::F(n) => Some(format!("F{n}")),
        _ => None,
    }
}

#[cfg(any(not(any(target_os = "macos", windows)), test))]
/// The freedesktop sound theme's name for a sound.
pub fn sound_name(sound: super::Sound) -> &'static str {
    match sound {
        super::Sound::Start => "audio-volume-change",
        super::Sound::Done => "complete",
        super::Sound::Error => "dialog-warning",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::Sound;
    use super::*;
    use crate::dictation::hotkey::Hotkey;

    fn menu(recent: usize, paused: bool) -> TrayMenu {
        TrayMenu {
            status: "Ready: hold Right Ctrl to dictate".into(),
            recent: (0..recent).map(|i| (format!("09:0{i}"), format!("text {i}"))).collect(),
            paused,
        }
    }

    #[test]
    fn tray_menu_items_and_their_actions() {
        let m = menu(2, true);
        let items = menu_items(&m);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Ready: hold Right Ctrl to dictate",
                "",
                "Recent (click to copy)",
                "09:00  text 0",
                "09:01  text 1",
                "",
                "Pause dictation",
                "Open Meeting Transcriber",
                "",
                "Quit Meeting Transcriber",
            ]
        );
        assert_eq!(items[0].kind, ItemKind::Text);
        assert_eq!(items[6].kind, ItemKind::Check(true));
        // Every id is its own and none is 0.
        let mut ids: Vec<u32> = items.iter().map(|i| i.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), items.len());
        assert!(!ids.contains(&0));
        assert_eq!(menu_action(items[4].id, &m), Some(TrayAction::Copy("text 1".into())));
        assert_eq!(menu_action(ID_PAUSE, &m), Some(TrayAction::TogglePause));
        assert_eq!(menu_action(ID_OPEN, &m), Some(TrayAction::OpenWindow));
        assert_eq!(menu_action(ID_QUIT, &m), Some(TrayAction::Quit));
        assert_eq!(menu_action(ID_STATUS, &m), None);
        assert_eq!(menu_action(RECENT + 5, &m), None);
        assert_eq!(menu_items(&menu(0, false))[2].label, "No dictations yet");
        assert_eq!(menu_items(&menu(0, false))[4].kind, ItemKind::Check(false));
    }

    #[test]
    fn menu_text_is_one_short_line() {
        assert_eq!(windows_menu_text("Q&A\tnotes"), "Q&&A notes");
        assert_eq!(shorten("Hello\n  world"), "Hello world");
        let long = "字".repeat(60);
        assert_eq!(shorten(&long), format!("{}…", "字".repeat(48)));
    }

    #[test]
    fn tray_icons_are_round_and_differ_by_state() {
        let icons = [
            TrayIcon::Loading,
            TrayIcon::Ready,
            TrayIcon::Recording,
            TrayIcon::Transcribing,
            TrayIcon::Paused,
            TrayIcon::Error,
        ];
        let drawn: Vec<Vec<[u8; 4]>> = icons.iter().map(|i| icon_rgba(*i, 32)).collect();
        for pixels in &drawn {
            assert_eq!(pixels.len(), 32 * 32);
            // Corners are transparent, the middle opaque.
            assert_eq!(pixels[0][3], 0);
            assert_eq!(pixels[32 * 32 - 1][3], 0);
            assert_eq!(pixels[16 * 32 + 16][3], 255);
        }
        for (i, a) in drawn.iter().enumerate() {
            for b in &drawn[i + 1..] {
                assert_ne!(a, b);
            }
        }
        // The microphone is white on the circle: its capsule is at (16, 12).
        let ready = &drawn[1];
        assert_eq!(ready[12 * 32 + 16], [255, 255, 255, 255]);
        assert_eq!(ready[12 * 32 + 5], [46, 160, 67, 255]);
        assert_eq!(icon_rgba(TrayIcon::Ready, 16).len(), 256);
    }

    #[test]
    fn login_entries_quote_the_program() {
        assert_eq!(
            run_command(Path::new(
                r"C:\Program Files\Meeting Transcriber\meeting-transcriber.exe"
            )),
            r#""C:\Program Files\Meeting Transcriber\meeting-transcriber.exe" --hidden"#
        );
        let entry = autostart_entry(Path::new("/home/me/.local/bin/meeting-transcriber"));
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(entry.contains("\nExec=\"/home/me/.local/bin/meeting-transcriber\" --hidden\n"));
        assert!(entry.contains("\nX-GNOME-Autostart-enabled=true\n"));
        assert_eq!(desktop_exec_arg("/opt/my apps/mt"), "\"/opt/my apps/mt\"");
        assert_eq!(desktop_exec_arg("/a\"b$c`d"), r#""/a\\"b\\$c\\`d""#);
        assert_eq!(desktop_exec_arg(r"/a\b"), r#""/a\\\\b""#);
        assert_eq!(desktop_exec_arg("/100%"), "\"/100%%\"");
    }

    #[test]
    fn dictate_commands() {
        assert_eq!(DictateCommand::parse("start\n"), Some(DictateCommand::Start));
        assert_eq!(DictateCommand::parse(" STOP "), Some(DictateCommand::Stop));
        assert_eq!(DictateCommand::parse("toggle"), Some(DictateCommand::Toggle));
        assert_eq!(DictateCommand::parse("down"), Some(DictateCommand::Start));
        assert_eq!(DictateCommand::parse(""), None);
        assert_eq!(DictateCommand::parse("start now"), None);
        for cmd in [DictateCommand::Start, DictateCommand::Stop, DictateCommand::Toggle] {
            assert_eq!(DictateCommand::parse(cmd.name()), Some(cmd));
        }
        use super::super::KeyEvent;
        assert_eq!(DictateCommand::Start.key_event(), KeyEvent::Down);
        assert_eq!(DictateCommand::Stop.key_event(), KeyEvent::Up);
        assert_eq!(DictateCommand::Toggle.key_event(), KeyEvent::Toggle);
        assert_eq!(DictateCommand::Toggle.done(), "toggled");
    }

    #[test]
    fn dictate_socket_is_private_to_the_user() {
        let data = Path::new("/home/me/.local/share/Meeting Transcriber");
        assert_eq!(
            socket_path(Some(Path::new("/run/user/1000")), data),
            PathBuf::from("/run/user/1000/meeting-transcriber.sock")
        );
        assert_eq!(socket_path(None, data), data.join("dictate.sock"));
        // A relative XDG_RUNTIME_DIR is invalid by the spec.
        assert_eq!(socket_path(Some(Path::new("run")), data), data.join("dictate.sock"));
    }

    #[test]
    fn linux_key_sources_and_paste_tools() {
        assert_eq!(key_sources(false, true), [KeySource::X11, KeySource::Portal]);
        assert_eq!(key_sources(true, true), [KeySource::Portal]);
        assert_eq!(key_sources(false, false), [KeySource::Portal]);
        let all = |_: &str| true;
        assert_eq!(paste_tools(false, true, all), [PasteTool::XTest]);
        assert_eq!(paste_tools(true, false, all), [PasteTool::Wtype, PasteTool::Ydotool]);
        assert_eq!(paste_tools(true, true, all), [PasteTool::Ydotool, PasteTool::Wtype]);
        assert_eq!(paste_tools(true, false, |t| t == "ydotool"), [PasteTool::Ydotool]);
        assert_eq!(paste_tools(true, true, |_| false), []);
    }

    #[test]
    fn x11_keysyms_to_key_codes() {
        // Two keysyms per key code, from code 8: `v` is on code 10.
        let keysyms = [0x61, 0x41, 0x62, 0x42, 0x76, 0x56, 0xffe3, 0];
        assert_eq!(keycode_for(0x76, 8, 2, &keysyms), Some(10));
        assert_eq!(keycode_for(0xffe3, 8, 2, &keysyms), Some(11));
        // Only the first keysym of each key counts: `V` is not looked up.
        assert_eq!(keycode_for(0x56, 8, 2, &keysyms), None);
        assert_eq!(keycode_for(0x76, 8, 0, &keysyms), Some(12));
    }

    #[test]
    fn portal_names() {
        assert_eq!(
            portal_request_path(":1.234", "mt_bind"),
            "/org/freedesktop/portal/desktop/request/1_234/mt_bind"
        );
        assert_eq!(portal_trigger(Hotkey::F(13)), Some("F13".into()));
        assert_eq!(portal_trigger(Hotkey::RightCtrl), None);
        assert_eq!(sound_name(Sound::Done), "complete");
    }
}
