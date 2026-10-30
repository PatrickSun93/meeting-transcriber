//! The menu bar item: an SF Symbol for dictation's state, and a menu with
//! the latest dictations (click to copy), Pause, Open and Quit. AppKit
//! objects live on the main thread, so this is made and updated from the
//! window's frame loop; the menu is rebuilt each time it opens.

use std::cell::RefCell;

use anyhow::{Context as _, Result};
use crossbeam_channel::Sender;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSColor, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::NSString;

use super::super::{TrayAction, TrayIcon, TrayMenu};

const TAG_PAUSE: isize = 1000;
const TAG_OPEN: isize = 1001;
const TAG_QUIT: isize = 1002;
/// Longest a dictation is shown in the menu, in characters.
const MENU_TEXT: usize = 48;

struct Ivars {
    menu: RefCell<TrayMenu>,
    actions: Sender<TrayAction>,
    /// Wakes the window's frame loop to act on an action.
    wake: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Target`
    // does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MeetingTranscriberTrayTarget"]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        #[unsafe(method(onItem:))]
        fn on_item(&self, item: &NSMenuItem) {
            self.clicked(item.tag());
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSMenuDelegate for Target {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.rebuild(menu);
        }
    }
);

impl Target {
    fn create(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSObject's plain init.
        unsafe { msg_send![super(this), init] }
    }

    fn clicked(&self, tag: isize) {
        let ivars = self.ivars();
        let action = match tag {
            TAG_PAUSE => TrayAction::TogglePause,
            TAG_OPEN => TrayAction::OpenWindow,
            TAG_QUIT => TrayAction::Quit,
            i => match ivars.menu.borrow().recent.get(i as usize) {
                Some((_, text)) => TrayAction::Copy(text.clone()),
                None => return,
            },
        };
        let _ = ivars.actions.send(action);
        (ivars.wake)();
    }

    fn rebuild(&self, menu: &NSMenu) {
        let mtm = self.mtm();
        let model = self.ivars().menu.borrow().clone();
        menu.removeAllItems();
        menu.addItem(&self.item(mtm, &model.status, None));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        if model.recent.is_empty() {
            menu.addItem(&self.item(mtm, "No dictations yet", None));
        } else {
            menu.addItem(&self.item(mtm, "Recent (click to copy)", None));
            for (i, (time, text)) in model.recent.iter().enumerate() {
                menu.addItem(&self.item(mtm, &format!("{time}  {}", shorten(text)), Some(i as isize)));
            }
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let pause = self.item(mtm, "Pause dictation", Some(TAG_PAUSE));
        pause.setState(if model.paused {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        menu.addItem(&pause);
        menu.addItem(&self.item(mtm, "Open Meeting Transcriber", Some(TAG_OPEN)));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&self.item(mtm, "Quit Meeting Transcriber", Some(TAG_QUIT)));
    }

    /// A menu item that calls `onItem:` with `tag`, or a greyed-out line.
    fn item(&self, mtm: MainThreadMarker, title: &str, tag: Option<isize>) -> Retained<NSMenuItem> {
        let action = tag.map(|_| sel!(onItem:));
        // SAFETY: `onItem:` is defined above and takes the item.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                action,
                &NSString::from_str(""),
            )
        };
        match tag {
            Some(tag) => {
                let target: &AnyObject = self.as_ref();
                // SAFETY: the target lives as long as the tray, which owns the menu.
                unsafe { item.setTarget(Some(target)) };
                item.setTag(tag);
            }
            None => item.setEnabled(false),
        }
        item
    }
}

/// One line, at most `MENU_TEXT` characters.
fn shorten(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(MENU_TEXT) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

pub struct Tray {
    item: Retained<NSStatusItem>,
    target: Retained<Target>,
    _menu: Retained<NSMenu>,
    shown: RefCell<Option<(TrayIcon, String)>>,
}

impl Tray {
    /// Adds the item to the menu bar. Must be called on the main thread,
    /// once the app is running.
    pub fn new(actions: Sender<TrayAction>, wake: Box<dyn Fn()>) -> Result<Self> {
        let mtm = MainThreadMarker::new().context("the menu bar item must be made on the main thread")?;
        let target = Target::create(
            mtm,
            Ivars {
                menu: RefCell::default(),
                actions,
                wake,
            },
        );
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        item.setMenu(Some(&menu));
        let tray = Self {
            item,
            target,
            _menu: menu,
            shown: RefCell::new(None),
        };
        tray.set(TrayIcon::Loading, "Meeting Transcriber", TrayMenu::default());
        Ok(tray)
    }

    /// Updates the icon and what the menu will show when next opened.
    pub fn set(&self, icon: TrayIcon, tooltip: &str, menu: TrayMenu) {
        *self.target.ivars().menu.borrow_mut() = menu;
        let state = (icon, tooltip.to_string());
        if self.shown.borrow().as_ref() == Some(&state) {
            return;
        }
        let Some(button) = self.item.button(self.target.mtm()) else {
            return;
        };
        let symbol = match icon {
            TrayIcon::Loading => "hourglass",
            TrayIcon::Ready => "mic",
            TrayIcon::Recording => "mic.fill",
            TrayIcon::Transcribing => "waveform",
            TrayIcon::Paused => "mic.slash",
            TrayIcon::Error => "exclamationmark.triangle",
        };
        let tip = NSString::from_str(tooltip);
        let image =
            NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(symbol), Some(&tip));
        button.setImage(image.as_deref());
        let red = (icon == TrayIcon::Recording).then(NSColor::systemRedColor);
        button.setContentTintColor(red.as_deref());
        button.setToolTip(Some(&tip));
        *self.shown.borrow_mut() = Some(state);
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

#[cfg(test)]
mod tests {
    use super::shorten;

    #[test]
    fn menu_text_is_one_short_line() {
        assert_eq!(shorten("Hello\n  world"), "Hello world");
        let long = "字".repeat(60);
        assert_eq!(shorten(&long), format!("{}…", "字".repeat(48)));
    }
}
