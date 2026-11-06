//! Screenshots on Linux: on X11, the monitor under the mouse (RandR) read
//! from the root window with `GetImage`; for a scrolling screenshot, a part
//! of it read again and again, and the wheel turned through XTEST. Wayland
//! lets no app read the screen this way, so there it is not supported yet.

use anyhow::{Context as _, Result, bail};
use x11rb::connection::Connection as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _, ImageFormat, ImageOrder};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::image::{Image, Packing};
use super::scrolling::Area;
use super::{Display, Shot};

/// The X server's mouse button for the wheel turned down.
const WHEEL_DOWN: u8 = 5;
const NO_XTEST: &str = "The X server has no XTEST extension, so the app cannot scroll: scroll by hand.";
const SCROLL_FAILED: &str = "The X server did not take the scroll, so scroll by hand.";

/// Why screenshots cannot be taken here, if they cannot.
pub fn unsupported() -> Option<&'static str> {
    crate::platform::wayland_session().then_some("Screenshots are not supported on Wayland yet; they work on X11.")
}

pub fn grab() -> Result<Shot> {
    if let Some(why) = unsupported() {
        bail!("{why}");
    }
    let (conn, screen) = x11rb::connect(None).context("Screenshots need an X11 display")?;
    let setup = conn.setup();
    let screen = setup.roots.get(screen).context("no X11 screen")?;
    let root = screen.root;
    let pointer = conn.query_pointer(root)?.reply()?;
    let (mx, my) = (i32::from(pointer.root_x), i32::from(pointer.root_y));
    let monitors = conn
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.monitors)
        .unwrap_or_default();
    let (x, y, width, height) = monitors
        .iter()
        .map(|m| (m.x, m.y, m.width, m.height))
        .find(|&(x, y, w, h)| {
            (i32::from(x)..i32::from(x) + i32::from(w)).contains(&mx)
                && (i32::from(y)..i32::from(y) + i32::from(h)).contains(&my)
        })
        .unwrap_or((0, 0, screen.width_in_pixels, screen.height_in_pixels));
    let image = read_root(&conn, screen, x, y, width, height)?;
    Ok(Shot {
        image,
        display: Display {
            origin_px: [i32::from(x), i32::from(y)],
            scale: 1.0,
        },
        windows: Vec::new(),
        taken: chrono::Local::now().naive_local(),
    })
}

/// The pixels of a part of the root window.
fn read_root(conn: &RustConnection, screen: &xproto::Screen, x: i16, y: i16, width: u16, height: u16) -> Result<Image> {
    let setup = conn.setup();
    let root = screen.root;
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, root, x, y, width, height, !0)?
        .reply()
        .context("the X server did not hand over the screen")?;
    let format = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == reply.depth)
        .context("the X server named no format for its pixels")?;
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|d| d.visuals.iter())
        .find(|v| v.visual_id == reply.visual)
        .context("the X server named no colours for its pixels")?;
    if format.bits_per_pixel != 32 {
        bail!(
            "the screen's pixels are {} bits each; screenshots read 32-bit screens",
            format.bits_per_pixel
        );
    }
    let pad = usize::from(format.scanline_pad.max(8)) / 8;
    let stride = (usize::from(width) * 4).div_ceil(pad) * pad;
    Image::from_packed(
        u32::from(width),
        u32::from(height),
        stride,
        &reply.data,
        Packing {
            red: visual.red_mask,
            green: visual.green_mask,
            blue: visual.blue_mask,
            big_endian: setup.image_byte_order == ImageOrder::MSB_FIRST,
        },
    )
}

/// A scrolling screenshot's area, read again and again, and the wheel
/// turned over it, through one connection to the X server.
pub struct Region {
    conn: RustConnection,
    screen: usize,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    xtest: bool,
}

impl Region {
    pub fn new(area: &Area) -> Result<Region> {
        if let Some(why) = unsupported() {
            bail!("{why}");
        }
        let (conn, screen) = x11rb::connect(None).context("Screenshots need an X11 display")?;
        conn.setup().roots.get(screen).context("no X11 screen")?;
        let xtest = conn
            .xtest_get_version(2, 2)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        let [left, top, right, bottom] = area.desktop();
        let coord = |v: i32| i16::try_from(v).context("the area is off the X11 screen");
        let size = |v: i32| u16::try_from(v).context("the area is off the X11 screen");
        Ok(Region {
            x: coord(left)?,
            y: coord(top)?,
            width: size(right - left)?,
            height: size(bottom - top)?,
            conn,
            screen,
            xtest,
        })
    }

    fn root(&self) -> xproto::Window {
        self.conn.setup().roots[self.screen].root
    }

    /// The area as it is now.
    pub fn grab(&mut self) -> Result<Image> {
        let screen = &self.conn.setup().roots[self.screen];
        read_root(&self.conn, screen, self.x, self.y, self.width, self.height)
    }

    /// Why the app cannot turn the wheel, if it cannot.
    pub fn cannot_scroll(&self) -> Option<&'static str> {
        (!self.xtest).then_some(NO_XTEST)
    }

    /// Where the mouse pointer is, in the root window's pixels.
    pub fn pointer(&self) -> Option<[i32; 2]> {
        let reply = self.conn.query_pointer(self.root()).ok()?.reply().ok()?;
        Some([i32::from(reply.root_x), i32::from(reply.root_y)])
    }

    /// Puts the pointer at `at`, where the wheel scrolls.
    pub fn move_pointer(&self, [x, y]: [i32; 2]) {
        let (Ok(x), Ok(y)) = (i16::try_from(x), i16::try_from(y)) else {
            return;
        };
        let _ = self.conn.warp_pointer(x11rb::NONE, self.root(), 0, 0, 0, 0, x, y);
        let _ = self.conn.flush();
    }

    /// Scrolls what is under the pointer down by about `distance` pixels:
    /// whole clicks of the wheel, each some three lines.
    pub fn scroll(&self, distance: u32) -> Result<(), &'static str> {
        let root = self.root();
        for _ in 0..(distance / 50).clamp(1, 10) {
            for kind in [xproto::BUTTON_PRESS_EVENT, xproto::BUTTON_RELEASE_EVENT] {
                self.conn
                    .xtest_fake_input(kind, WHEEL_DOWN, x11rb::CURRENT_TIME, root, 0, 0, 0)
                    .map_err(|_| SCROLL_FAILED)?;
            }
        }
        self.conn.flush().map_err(|_| SCROLL_FAILED)
    }
}

/// The scrolling windows stay outside the area, so they are not in it.
pub fn hide_from_captures(_title: &str) -> bool {
    true
}

/// Nothing to remember: the window manager hands the focus back.
pub fn remember_front_app() {}

pub fn restore_front_app() {}

/// The window manager keeps an always-on-top window in front by itself.
pub fn raise_overlay(_title: &str, _focus: bool) -> bool {
    true
}
