//! The pictures a meeting's notes link (`![…](…)`, as screenshots are
//! added), shown small above the notes. Each is read and shrunk on a thread
//! of its own, and kept as a texture while the notes still link it; a click
//! opens it in the system's viewer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, Sense, Ui, vec2};

use super::platform;
use crate::capture::files::{image_links, link_path};
use crate::capture::image::Image;

/// How tall a thumbnail is, and at most how wide, in points.
const HEIGHT: f32 = 64.0;
const MAX_WIDTH: f32 = 160.0;
/// The longest side kept of a picture, in pixels: sharp at 2×.
const SIDE: u32 = 320;

enum Thumb {
    Loading,
    Ready(egui::TextureHandle),
    /// Not there, or not a picture this app reads.
    Unreadable,
}

/// A picture read (or not) on its thread.
type Loaded = (PathBuf, Option<Image>);

pub(super) struct Thumbnails {
    cache: HashMap<PathBuf, Thumb>,
    loaded: (Sender<Loaded>, Receiver<Loaded>),
}

impl Default for Thumbnails {
    fn default() -> Self {
        Self {
            cache: HashMap::new(),
            loaded: crossbeam_channel::unbounded(),
        }
    }
}

impl Thumbnails {
    /// Shows the PNG pictures `notes` link, relative to `dir` (the meeting
    /// file's folder). Nothing when there are none.
    pub fn show(&mut self, ui: &mut Ui, dir: &Path, notes: &str) {
        for (path, image) in self.loaded.1.try_iter() {
            let thumb = match image {
                Some(image) => Thumb::Ready(ui.ctx().load_texture(
                    path.to_string_lossy(),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [image.width as usize, image.height as usize],
                        &image.rgba,
                    ),
                    egui::TextureOptions::LINEAR,
                )),
                None => Thumb::Unreadable,
            };
            if let Some(slot) = self.cache.get_mut(&path) {
                *slot = thumb;
            }
        }
        let mut paths: Vec<PathBuf> = Vec::new();
        for link in image_links(notes) {
            let path = link_path(dir, &link);
            let png = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
            if png && !paths.contains(&path) {
                paths.push(path);
            }
        }
        // Textures of pictures no longer linked are let go.
        self.cache.retain(|p, _| paths.contains(p));
        if paths.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for path in &paths {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let thumb = self.cache.entry(path.clone()).or_insert_with(|| {
                    load(path.clone(), self.loaded.0.clone(), ui.ctx().clone());
                    Thumb::Loading
                });
                match thumb {
                    Thumb::Loading => {
                        ui.add_sized([HEIGHT, HEIGHT], egui::Spinner::new());
                    }
                    Thumb::Ready(texture) => {
                        let size = texture.size_vec2();
                        let scale = (HEIGHT / size.y.max(1.0)).min(MAX_WIDTH / size.x.max(1.0));
                        let image = egui::Image::new((texture.id(), size * scale))
                            .corner_radius(4)
                            .sense(Sense::click());
                        if ui
                            .add(image)
                            .on_hover_text(format!("{name}: click to open"))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            platform::open_file(path);
                        }
                    }
                    Thumb::Unreadable => {
                        ui.add_sized(vec2(HEIGHT, HEIGHT), egui::Label::new("?"))
                            .on_hover_text(format!("{name} is missing or can't be read"));
                    }
                }
            }
        });
        ui.add_space(6.0);
    }
}

/// Reads and shrinks the picture at `path` on a thread of its own.
fn load(path: PathBuf, done: Sender<Loaded>, ctx: egui::Context) {
    let (key, failed) = (path.clone(), done.clone());
    let spawned = std::thread::Builder::new().name("thumbnail".into()).spawn(move || {
        let image = std::fs::read(&path)
            .ok()
            .and_then(|bytes| Image::from_png(&bytes).ok())
            .map(|image| image.shrunk_to(SIDE));
        let _ = done.send((path, image));
        ctx.request_repaint();
    });
    if spawned.is_err() {
        // Shown as unreadable rather than loading for ever.
        let _ = failed.send((key, None));
    }
}
