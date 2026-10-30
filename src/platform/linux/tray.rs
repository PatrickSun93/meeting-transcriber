//! The tray icon, as a StatusNotifierItem over D-Bus: KDE Plasma, Cinnamon,
//! XFCE, LXQt, Budgie and others show it, GNOME with the AppIndicator
//! extension (Ubuntu has it on). A coloured microphone for dictation's
//! state; a click opens the window, and its menu (`com.canonical.dbusmenu`)
//! has the latest dictations (click to copy), Pause, Open and Quit.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context as _, Result};
use crossbeam_channel::Sender;
use zbus::names::BusName;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::super::shared::{ItemKind, MenuItem, icon_rgba, menu_action, menu_items};
use super::super::{TrayAction, TrayIcon, TrayMenu};

const ITEM_PATH: &str = "/StatusNotifierItem";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
/// Icon sizes offered; the desktop picks the closest.
const SIZES: [u32; 3] = [22, 32, 64];

/// `a(iiay)`: width, height and ARGB32 pixels in network byte order.
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
/// A D-Bus menu item: id, properties, children (`(ia{sv}av)`).
type Layout = (i32, HashMap<String, Value<'static>>, Vec<Value<'static>>);

/// What the tray shows, as `Tray::set` last said.
struct Shown {
    icon: TrayIcon,
    tooltip: String,
    menu: TrayMenu,
    /// Goes up with every change to the menu, as the D-Bus menu wants.
    revision: u32,
}

#[derive(Clone)]
struct Shared {
    shown: Arc<Mutex<Shown>>,
    actions: Sender<TrayAction>,
    /// Wakes the window's frame loop to act on an action.
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn shown(&self) -> MutexGuard<'_, Shown> {
        self.shown.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn act(&self, action: TrayAction) {
        let _ = self.actions.send(action);
        (self.wake)();
    }
}

/// The StatusNotifierItem.
struct Item(Shared);

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "meeting-transcriber"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "Meeting Transcriber"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        pixmaps(self.0.shown().icon)
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        let tooltip = self.0.shown().tooltip.clone();
        (String::new(), Vec::new(), "Meeting Transcriber".into(), tooltip)
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        ObjectPath::from_static_str_unchecked(MENU_PATH).into()
    }

    fn activate(&self, _x: i32, _y: i32) {
        self.0.act(TrayAction::OpenWindow);
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {
        self.0.act(TrayAction::OpenWindow);
    }

    /// The desktop shows the menu itself, from `Menu`.
    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: String) {}
}

/// The item's menu.
struct Menu(Shared);

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    /// The whole menu: it is one level deep, so `parent_id` and the depth
    /// change nothing but the root asked for.
    fn get_layout(&self, parent_id: i32, _recursion_depth: i32, _property_names: Vec<String>) -> (u32, Layout) {
        let shown = self.0.shown();
        (shown.revision, layout(parent_id, &shown.menu))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, Value<'static>>)> {
        let items = menu_items(&self.0.shown().menu);
        items
            .iter()
            .filter(|item| ids.is_empty() || ids.contains(&(item.id as i32)))
            .map(|item| (item.id as i32, properties(item)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let items = menu_items(&self.0.shown().menu);
        let value = items
            .iter()
            .find(|item| item.id as i32 == id)
            .and_then(|item| properties(item).remove(&name))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no property {name} on item {id}")))?;
        OwnedValue::try_from(value).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        if event_id == "clicked" {
            self.clicked(id);
        }
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event_id, _, _) in &events {
            if event_id == "clicked" {
                self.clicked(*id);
            }
        }
        Vec::new()
    }

    /// The menu is always current, so it never needs updating first.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
}

impl Menu {
    fn clicked(&self, id: i32) {
        let menu = self.0.shown().menu.clone();
        if let Some(action) = u32::try_from(id).ok().and_then(|id| menu_action(id, &menu)) {
            self.0.act(action);
        }
    }
}

/// The menu under `parent_id`: the root (0) holds every item.
fn layout(parent_id: i32, menu: &TrayMenu) -> Layout {
    let children = if parent_id == 0 {
        menu_items(menu)
            .iter()
            .map(|item| Value::from((item.id as i32, properties(item), Vec::<Value<'static>>::new())))
            .collect()
    } else {
        Vec::new()
    };
    let mut root = HashMap::new();
    root.insert("children-display".to_string(), Value::from("submenu"));
    (parent_id, root, children)
}

/// An item's D-Bus menu properties.
fn properties(item: &MenuItem) -> HashMap<String, Value<'static>> {
    let mut props: HashMap<String, Value<'static>> = HashMap::new();
    let mut set = |key: &str, value: Value<'static>| {
        props.insert(key.to_string(), value);
    };
    match item.kind {
        ItemKind::Separator => set("type", Value::from("separator")),
        ItemKind::Text => {
            set("label", Value::from(item.label.clone()));
            set("enabled", Value::from(false));
        }
        ItemKind::Action => set("label", Value::from(item.label.clone())),
        ItemKind::Check(on) => {
            set("label", Value::from(item.label.clone()));
            set("toggle-type", Value::from("checkmark"));
            set("toggle-state", Value::from(i32::from(on)));
        }
    }
    props
}

/// The state's icon at each offered size, as ARGB32 in network byte order.
fn pixmaps(icon: TrayIcon) -> Pixmaps {
    SIZES
        .iter()
        .map(|&size| {
            let bytes = icon_rgba(icon, size)
                .into_iter()
                .flat_map(|[r, g, b, a]| [a, r, g, b])
                .collect();
            (size as i32, size as i32, bytes)
        })
        .collect()
}

pub struct Tray {
    conn: zbus::blocking::Connection,
    shared: Shared,
    last: RefCell<Option<(TrayIcon, String)>>,
}

impl Tray {
    /// Puts the icon in the desktop's tray. Fails where the desktop shows
    /// none (no StatusNotifierWatcher on the session bus).
    pub fn new(actions: Sender<TrayAction>, wake: Box<dyn Fn() + Send + Sync>) -> Result<Self> {
        let shared = Shared {
            shown: Arc::new(Mutex::new(Shown {
                icon: TrayIcon::Loading,
                tooltip: "Meeting Transcriber".into(),
                menu: TrayMenu::default(),
                revision: 1,
            })),
            actions,
            wake: Arc::from(wake),
        };
        let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        let conn = zbus::blocking::connection::Builder::session()?
            .name(name.as_str())?
            .serve_at(ITEM_PATH, Item(shared.clone()))?
            .serve_at(MENU_PATH, Menu(shared.clone()))?
            .build()
            .context("could not reach the session bus")?;
        let watcher = zbus::blocking::Proxy::new(
            &conn,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
        )?;
        watcher
            .call_method("RegisterStatusNotifierItem", &(name.as_str(),))
            .context("the desktop shows no tray icons")?;
        Ok(Self {
            conn,
            shared,
            last: RefCell::new(None),
        })
    }

    /// Updates the icon and what the menu shows, telling the desktop what
    /// changed.
    pub fn set(&self, icon: TrayIcon, tooltip: &str, menu: TrayMenu) {
        let look = (icon, tooltip.to_string());
        let look_changed = self.last.borrow().as_ref() != Some(&look);
        let revision = {
            let mut shown = self.shared.shown();
            shown.icon = icon;
            shown.tooltip = tooltip.to_string();
            if shown.menu == menu {
                None
            } else {
                shown.menu = menu;
                shown.revision += 1;
                Some(shown.revision)
            }
        };
        let mut sent = Vec::new();
        if look_changed {
            *self.last.borrow_mut() = Some(look);
            for signal in ["NewIcon", "NewToolTip"] {
                sent.push(
                    self.conn
                        .emit_signal(None::<BusName>, ITEM_PATH, ITEM_INTERFACE, signal, &()),
                );
            }
        }
        if let Some(revision) = revision {
            // The root (0) and everything under it.
            let body = (revision, 0i32);
            sent.push(
                self.conn
                    .emit_signal(None::<BusName>, MENU_PATH, MENU_INTERFACE, "LayoutUpdated", &body),
            );
        }
        for e in sent.into_iter().filter_map(Result::err) {
            eprintln!("tray: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::Type as _;

    use super::*;

    #[test]
    fn menu_layout_has_the_d_bus_menu_types() {
        assert_eq!(<(u32, Layout)>::SIGNATURE.to_string(), "(u(ia{sv}av))");
        let menu = TrayMenu {
            status: "Ready".into(),
            recent: vec![("09:00".into(), "hello".into())],
            paused: true,
        };
        let (id, root, children) = layout(0, &menu);
        assert_eq!(id, 0);
        assert!(root.contains_key("children-display"));
        assert_eq!(children.len(), menu_items(&menu).len());
        assert_eq!(children[0].value_signature().to_string(), "(ia{sv}av)");
        assert!(layout(5, &menu).2.is_empty());
        let pause = menu_items(&menu)
            .into_iter()
            .find(|i| i.label == "Pause dictation")
            .unwrap();
        let props = properties(&pause);
        assert_eq!(props["toggle-type"], Value::from("checkmark"));
        assert_eq!(props["toggle-state"], Value::from(1));
    }

    #[test]
    fn pixmaps_are_argb() {
        let maps = pixmaps(TrayIcon::Ready);
        assert_eq!(maps.len(), SIZES.len());
        for (w, h, bytes) in &maps {
            assert_eq!(bytes.len(), (w * h * 4) as usize);
        }
        // The first pixel is a clear corner: alpha comes first.
        assert_eq!(maps[0].2[0], 0);
    }
}
