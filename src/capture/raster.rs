//! Drawing the marks onto the picture itself, at its full resolution, once
//! a screenshot is kept or pinned: lines and outlines with tiny-skia, text
//! with ab_glyph in the fonts the overlay shows it in (egui's own, then the
//! system's for the scripts those lack), and mosaic blocks from the
//! picture's own pixels.

use ab_glyph::{Font, FontArc, FontVec, GlyphId, PxScale, ScaleFont, point};
use eframe::egui::{self, Pos2, Rect};
use tiny_skia::{FillRule, IntSize, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use super::annotate::{Mark, Rgba, Shape, arrow_head, mosaic_blocks};
use super::image::Image;

/// The fonts text is drawn in: each character in the first that has it.
#[derive(Clone)]
pub struct Fonts {
    /// Each with how much larger or smaller egui shows it than its size.
    faces: Vec<(FontArc, f32)>,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fonts({})", self.faces.len())
    }
}

impl Fonts {
    /// egui's own proportional fonts, in egui's order: Ubuntu Light, then
    /// its icons and emoji.
    pub fn builtin() -> Fonts {
        let definitions = egui::FontDefinitions::default();
        let names = definitions.families.get(&egui::FontFamily::Proportional);
        let faces = names
            .into_iter()
            .flatten()
            .filter_map(|name| definitions.font_data.get(name))
            .filter_map(|data| {
                let bytes: &[u8] = (*data.font).as_ref();
                let font = FontVec::try_from_vec_and_index(bytes.to_vec(), data.index).ok()?;
                Some((FontArc::from(font), data.tweak.scale))
            })
            .collect();
        Fonts { faces }
    }

    /// Adds a font file (its first face), tried after the others; false
    /// when it is not one ab_glyph reads.
    pub fn add(&mut self, bytes: Vec<u8>) -> bool {
        match FontVec::try_from_vec_and_index(bytes, 0) {
            Ok(font) => {
                self.faces.push((FontArc::from(font), 1.0));
                true
            }
            Err(_) => false,
        }
    }

    /// Whether some font has every character of `text` that shows.
    pub fn covers(&self, text: &str) -> bool {
        text.chars()
            .filter(|c| !c.is_whitespace() && !c.is_control())
            .all(|c| self.face_for(c).is_some())
    }

    /// The first font with a glyph for `c`, by its place in the list.
    fn face_for(&self, c: char) -> Option<usize> {
        self.faces.iter().position(|(font, _)| font.glyph_id(c).0 != 0)
    }
}

/// The picture with the marks drawn on it, in the order drawn. The marks
/// are in the picture's pixels. A mosaic takes its colours from the picture
/// as it was, under every mark, as the overlay shows it.
pub fn render(image: &Image, marks: &[Mark], fonts: &Fonts) -> Image {
    let Some(size) = IntSize::from_wh(image.width, image.height) else {
        return image.clone();
    };
    let Some(mut pixmap) = Pixmap::from_vec(premultiplied(&image.rgba), size) else {
        return image.clone();
    };
    for mark in marks {
        let [r, g, b, a] = mark.color;
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, a);
        paint.anti_alias = true;
        let stroke = Stroke {
            width: mark.size.max(0.5),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        let path = match &mark.shape {
            Shape::Rectangle(rect) => tiny_rect(*rect).map(|r| {
                let mut builder = PathBuilder::new();
                builder.push_rect(r);
                builder
            }),
            Shape::Ellipse(rect) => tiny_rect(*rect).map(|r| {
                let mut builder = PathBuilder::new();
                builder.push_oval(r);
                builder
            }),
            Shape::Arrow { from, to } => {
                if let Some(head) = arrow_head(*from, *to, mark.size) {
                    let mut tip = PathBuilder::new();
                    tip.move_to(head.points[0].x, head.points[0].y);
                    tip.line_to(head.points[1].x, head.points[1].y);
                    tip.line_to(head.points[2].x, head.points[2].y);
                    tip.close();
                    if let Some(tip) = tip.finish() {
                        pixmap.fill_path(&tip, &paint, FillRule::Winding, Transform::identity(), None);
                    }
                    Some(polyline(&[*from, head.shaft_end]))
                } else {
                    None
                }
            }
            Shape::Pen(points) => match points.as_slice() {
                [] => None,
                [dot] => {
                    if let Some(circle) = PathBuilder::from_circle(dot.x, dot.y, stroke.width / 2.0) {
                        pixmap.fill_path(&circle, &paint, FillRule::Winding, Transform::identity(), None);
                    }
                    None
                }
                _ => Some(polyline(points)),
            },
            Shape::Text { at, text, .. } => {
                draw_text(&mut pixmap, fonts, *at, text, mark.size, mark.color);
                None
            }
            Shape::Mosaic(rect) => {
                for (cell, color) in mosaic_blocks(image, *rect, mark.size) {
                    fill_cell(&mut pixmap, cell, color);
                }
                None
            }
        };
        if let Some(path) = path.and_then(PathBuilder::finish) {
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
    Image {
        width: image.width,
        height: image.height,
        rgba: straight(pixmap.data()),
    }
}

fn tiny_rect(rect: Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_ltrb(rect.min.x, rect.min.y, rect.max.x, rect.max.y)
}

fn polyline(points: &[Pos2]) -> PathBuilder {
    let mut builder = PathBuilder::new();
    if let [first, rest @ ..] = points {
        builder.move_to(first.x, first.y);
        for p in rest {
            builder.line_to(p.x, p.y);
        }
    }
    builder
}

/// Paints a mosaic block, in place of what is there.
fn fill_cell(pixmap: &mut Pixmap, cell: super::select::PxRect, color: Rgba) {
    let width = pixmap.width() as usize;
    let premultiplied = premultiply(color);
    let data = pixmap.data_mut();
    for y in cell.y..cell.y + cell.height {
        let start = (y as usize * width + cell.x as usize) * 4;
        for px in data[start..start + cell.width as usize * 4].as_chunks_mut::<4>().0 {
            *px = premultiplied;
        }
    }
}

/// One line of text with its top-left corner at `at`, `size` pixels to the
/// em as egui sizes fonts, its baseline where egui puts it: the first
/// font's ascent down.
fn draw_text(pixmap: &mut Pixmap, fonts: &Fonts, at: Pos2, text: &str, size: f32, color: Rgba) {
    let Some((main, main_tweak)) = fonts.faces.first() else {
        return;
    };
    let em = |font: &FontArc, tweak: f32| size * tweak / font.units_per_em().unwrap_or(1000.0);
    let baseline = at.y + (main.ascent_unscaled() * em(main, *main_tweak)).round();
    let mut x = at.x;
    let mut last: Option<(usize, GlyphId)> = None;
    for c in text.chars().filter(|c| !c.is_control()) {
        let face = fonts.face_for(c).unwrap_or(0);
        let (font, tweak) = &fonts.faces[face];
        // ab_glyph scales by the font's height; egui by its em.
        let scale = PxScale::from(font.height_unscaled() * em(font, *tweak));
        let scaled = font.as_scaled(scale);
        let id = font.glyph_id(c);
        if let Some((before, previous)) = last
            && before == face
        {
            x += scaled.kern(previous, id);
        }
        let glyph = id.with_scale_and_position(scale, point(x, baseline));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            let (left, top) = (bounds.min.x as i64, bounds.min.y as i64);
            outlined.draw(|gx, gy, coverage| {
                blend(pixmap, left + i64::from(gx), top + i64::from(gy), color, coverage);
            });
        }
        x += scaled.h_advance(id);
        last = Some((face, id));
    }
}

/// `color` over the pixel at (`x`, `y`), as much as `coverage` (0 to 1).
fn blend(pixmap: &mut Pixmap, x: i64, y: i64, color: Rgba, coverage: f32) {
    let (width, height) = (i64::from(pixmap.width()), i64::from(pixmap.height()));
    if x < 0 || y < 0 || x >= width || y >= height {
        return;
    }
    let alpha = coverage.clamp(0.0, 1.0) * f32::from(color[3]) / 255.0;
    if alpha <= 0.0 {
        return;
    }
    let i = ((y * width + x) * 4) as usize;
    let px = &mut pixmap.data_mut()[i..i + 4];
    for c in 0..3 {
        let value = f32::from(color[c]) * alpha + f32::from(px[c]) * (1.0 - alpha);
        px[c] = value.round().min(255.0) as u8;
    }
    px[3] = (255.0 * alpha + f32::from(px[3]) * (1.0 - alpha)).round().min(255.0) as u8;
}

/// A straight-alpha colour as tiny-skia keeps it: multiplied by its alpha.
fn premultiply([r, g, b, a]: Rgba) -> Rgba {
    let times = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
    [times(r), times(g), times(b), a]
}

fn premultiplied(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>().0.iter().flat_map(|px| premultiply(*px)).collect()
}

/// Back from tiny-skia's pixels to straight alpha.
fn straight(data: &[u8]) -> Vec<u8> {
    data.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| {
            if a == 255 || a == 0 {
                [r, g, b, a]
            } else {
                let undo = |c: u8| ((u16::from(c) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8;
                [undo(r), undo(g), undo(b), a]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};

    const RED: Rgba = [255, 0, 0, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    fn white(width: u32, height: u32) -> Image {
        Image {
            width,
            height,
            rgba: WHITE.repeat((width * height) as usize),
        }
    }

    fn at(image: &Image, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * image.width + x) * 4) as usize;
        image.rgba[i..i + 4].try_into().unwrap()
    }

    fn red_ish(px: [u8; 4]) -> bool {
        px[0] > 200 && px[1] < 80 && px[2] < 80
    }

    fn mark(shape: Shape, size: f32) -> Mark {
        Mark {
            shape,
            color: RED,
            size,
        }
    }

    #[test]
    fn a_red_rectangle_outlines_and_leaves_the_rest() {
        let image = white(40, 30);
        let rect = Rect::from_min_max(pos2(10.0, 5.0), pos2(30.0, 25.0));
        let out = render(
            &image,
            &[mark(Shape::Rectangle(rect), 4.0)],
            &Fonts { faces: Vec::new() },
        );
        assert_eq!((out.width, out.height), (40, 30));
        assert!(red_ish(at(&out, 10, 15)), "the left edge: {:?}", at(&out, 10, 15));
        assert!(red_ish(at(&out, 20, 24)), "the bottom edge");
        assert!(red_ish(at(&out, 8, 15)), "half the width falls outside");
        assert_eq!(at(&out, 20, 15), WHITE, "the inside");
        assert_eq!(at(&out, 2, 2), WHITE, "outside");
        assert_eq!(at(&out, 6, 15), WHITE, "beyond the line");
        assert_eq!(image, white(40, 30), "the picture itself is left alone");
    }

    #[test]
    fn ellipses_pens_and_see_through_colours() {
        let image = white(50, 50);
        let ellipse = mark(
            Shape::Ellipse(Rect::from_min_max(pos2(5.0, 5.0), pos2(45.0, 45.0))),
            3.0,
        );
        let pen = Mark {
            shape: Shape::Pen(vec![pos2(0.0, 2.0), pos2(49.0, 2.0)]),
            color: [0, 0, 255, 128],
            size: 2.0,
        };
        let out = render(&image, &[ellipse, pen], &Fonts { faces: Vec::new() });
        assert!(red_ish(at(&out, 25, 5)), "the top of the ellipse");
        assert!(red_ish(at(&out, 45, 25)), "its right");
        assert_eq!(at(&out, 25, 25), WHITE, "its middle");
        assert_eq!(at(&out, 6, 6), WHITE, "its box's corner");
        let half_blue = at(&out, 25, 2);
        assert!(
            (120..=135).contains(&half_blue[0]) && half_blue[2] == 255 && half_blue[3] == 255,
            "half blue over white: {half_blue:?}"
        );
    }

    #[test]
    fn an_arrow_has_a_head_at_its_end() {
        let image = white(100, 40);
        let arrow = mark(
            Shape::Arrow {
                from: pos2(5.0, 20.0),
                to: pos2(90.0, 20.0),
            },
            2.0,
        );
        let out = render(&image, &[arrow], &Fonts { faces: Vec::new() });
        assert!(red_ish(at(&out, 40, 20)), "the line");
        assert_eq!(at(&out, 40, 24), WHITE, "beside the line");
        // The head, 14 long and 7.7 to each side of the line at its base.
        assert!(red_ish(at(&out, 80, 24)), "the head is wider: {:?}", at(&out, 80, 24));
        assert!(red_ish(at(&out, 80, 15)));
        assert!(red_ish(at(&out, 88, 20)), "near the tip");
        assert_eq!(at(&out, 94, 20), WHITE, "past the tip");
    }

    #[test]
    fn mosaic_averages_blocks_of_the_picture() {
        // Stripes one pixel wide: black and white.
        let rgba = (0..8 * 8)
            .flat_map(|i| if i % 2 == 0 { [0, 0, 0, 255] } else { WHITE })
            .collect();
        let image = Image {
            width: 8,
            height: 8,
            rgba,
        };
        let mosaic = mark(Shape::Mosaic(Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 8.0))), 4.0);
        let out = render(&image, &[mosaic], &Fonts { faces: Vec::new() });
        for (x, y) in [(0, 0), (1, 0), (3, 3), (2, 7)] {
            assert_eq!(at(&out, x, y), [128, 128, 128, 255], "({x}, {y})");
        }
        assert_eq!(at(&out, 4, 0), [0, 0, 0, 255], "outside it is untouched");
        assert_eq!(at(&out, 5, 0), WHITE);

        // A mosaic over a mark shows the picture under it, as the overlay does.
        let line = mark(Shape::Pen(vec![pos2(0.0, 4.0), pos2(8.0, 4.0)]), 2.0);
        let covered = render(
            &image,
            &[
                line,
                mark(Shape::Mosaic(Rect::from_min_max(pos2(0.0, 0.0), pos2(8.0, 8.0))), 4.0),
            ],
            &Fonts { faces: Vec::new() },
        );
        assert_eq!(at(&covered, 0, 4), [128, 128, 128, 255]);
    }

    #[test]
    fn text_is_drawn_in_its_box() {
        let fonts = Fonts::builtin();
        assert!(fonts.covers("Hello, world 123"));
        assert!(!fonts.covers("你好"), "egui's fonts have no Chinese");
        let image = white(120, 40);
        let text = mark(
            Shape::Text {
                at: pos2(10.0, 8.0),
                text: "Hi".into(),
                extent: vec2(20.0, 24.0),
            },
            20.0,
        );
        let out = render(&image, &[text], &fonts);
        let inked = |x0: u32, x1: u32, y0: u32, y1: u32| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| at(&out, x, y) != WHITE)
                .count()
        };
        assert!(inked(10, 40, 8, 32) > 40, "the letters are there");
        assert_eq!(inked(0, 10, 0, 40), 0, "nothing left of the text");
        assert_eq!(inked(0, 120, 0, 8), 0, "nothing above it");
        assert_eq!(inked(45, 120, 0, 40), 0, "nothing past it");
        assert!(red_ish(
            (8..32)
                .flat_map(|y| (10..40).map(move |x| (x, y)))
                .map(|(x, y)| at(&out, x, y))
                .min_by_key(|px| px[1])
                .unwrap()
        ));

        // With no fonts at all nothing is drawn, and a file that is no
        // font is turned down.
        let unknown = mark(
            Shape::Text {
                at: pos2(10.0, 8.0),
                text: "你".into(),
                extent: vec2(20.0, 24.0),
            },
            20.0,
        );
        assert_eq!(render(&image, &[unknown], &Fonts { faces: Vec::new() }), image);
        let mut more = Fonts::builtin();
        assert!(!more.add(b"not a font".to_vec()));
    }

    #[test]
    fn colours_with_alpha_round_trip() {
        let px = [200, 100, 50, 128];
        let back = straight(&premultiply(px));
        assert!(back.iter().zip(px).all(|(a, b)| a.abs_diff(b) <= 2), "{back:?}");
        assert_eq!(straight(&premultiply(RED)), RED);
    }
}
