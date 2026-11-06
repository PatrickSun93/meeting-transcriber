//! Screenshots on Linux: on X11, the monitor under the mouse (RandR) read
//! from the root window with `GetImage`. Wayland lets no app read the
//! screen this way, so there it is not supported yet.

use anyhow::{Context as _, Result, bail};
use x11rb::connection::Connection as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};

use super::image::{Image, Packing};
use super::{Display, Shot};

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
    let image = Image::from_packed(
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
    )?;
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

/// Nothing to remember: the window manager hands the focus back.
pub fn remember_front_app() {}

pub fn restore_front_app() {}

/// The window manager keeps an always-on-top window in front by itself.
pub fn raise_overlay(_title: &str) -> bool {
    true
}
