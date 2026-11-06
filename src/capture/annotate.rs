//! Drawing on a screenshot before it is kept: the marks (rectangles,
//! ellipses, arrows, pen strokes, text and mosaic), the list of them with
//! undo and redo, finding the one under the pointer, and the geometry the
//! overlay and the rasterizer (`raster.rs`) share. Everything is in the
//! screenshot's pixels.

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

use super::image::Image;
use super::select::PxRect;

/// A colour: red, green, blue, alpha.
pub type Rgba = [u8; 4];

/// A drawing tool on the overlay's toolbar, making one kind of mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Rectangle,
    Ellipse,
    Arrow,
    Pen,
    Text,
    Mosaic,
}

impl Tool {
    /// Left to right on the toolbar.
    pub const ALL: [Tool; 6] = [
        Tool::Rectangle,
        Tool::Ellipse,
        Tool::Arrow,
        Tool::Pen,
        Tool::Text,
        Tool::Mosaic,
    ];

    /// Its place in `ALL`.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// What a mark is, and where.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// An outline between two corners.
    Rectangle(Rect),
    /// An outline inside a rectangle.
    Ellipse(Rect),
    /// A line with a head at `to`.
    Arrow { from: Pos2, to: Pos2 },
    /// A freehand stroke through these points.
    Pen(Vec<Pos2>),
    /// One line of text, its top-left corner at `at`; `extent` is its size
    /// as laid out on the overlay.
    Text { at: Pos2, text: String, extent: Vec2 },
    /// The picture in blocks, each one colour: its average.
    Mosaic(Rect),
}

/// One thing drawn on the screenshot.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub shape: Shape,
    pub color: Rgba,
    /// The line's width; for text the font size; for mosaic a block's side.
    pub size: f32,
}

impl Mark {
    /// Too small to keep: a click rather than a drag.
    pub fn is_empty(&self) -> bool {
        match &self.shape {
            Shape::Rectangle(r) | Shape::Ellipse(r) | Shape::Mosaic(r) => r.width() < 2.0 || r.height() < 2.0,
            Shape::Arrow { from, to } => from.distance(*to) < 2.0,
            Shape::Pen(points) => points.is_empty(),
            Shape::Text { text, .. } => text.trim().is_empty(),
        }
    }

    /// The same mark moved by `by`.
    pub fn translated(&self, by: Vec2) -> Mark {
        let shape = match &self.shape {
            Shape::Rectangle(r) => Shape::Rectangle(r.translate(by)),
            Shape::Ellipse(r) => Shape::Ellipse(r.translate(by)),
            Shape::Mosaic(r) => Shape::Mosaic(r.translate(by)),
            Shape::Arrow { from, to } => Shape::Arrow {
                from: *from + by,
                to: *to + by,
            },
            Shape::Pen(points) => Shape::Pen(points.iter().map(|p| *p + by).collect()),
            Shape::Text { at, text, extent } => Shape::Text {
                at: *at + by,
                text: text.clone(),
                extent: *extent,
            },
        };
        Mark { shape, ..*self }
    }

    /// Everything the mark covers.
    pub fn bounds(&self) -> Rect {
        let half = self.size / 2.0;
        match &self.shape {
            Shape::Rectangle(r) | Shape::Ellipse(r) => r.expand(half),
            Shape::Mosaic(r) => *r,
            Shape::Arrow { from, to } => {
                let mut bounds = Rect::from_two_pos(*from, *to);
                if let Some(head) = arrow_head(*from, *to, self.size) {
                    for p in head.points {
                        bounds.extend_with(p);
                    }
                }
                bounds.expand(half)
            }
            Shape::Pen(points) => Rect::from_points(points).expand(half),
            Shape::Text { at, extent, .. } => Rect::from_min_size(*at, *extent),
        }
    }

    /// Whether `p` is on the mark, give or take `reach`: on its lines (a
    /// mosaic's edge), or anywhere in a text.
    pub fn hit(&self, p: Pos2, reach: f32) -> bool {
        let near = reach + self.size / 2.0;
        match &self.shape {
            Shape::Rectangle(r) => outline_distance(*r, p) <= near,
            Shape::Mosaic(r) => outline_distance(*r, p) <= reach,
            Shape::Ellipse(r) => ellipse_distance(*r, p) <= near,
            Shape::Arrow { from, to } => {
                segment_distance(*from, *to, p) <= near
                    || arrow_head(*from, *to, self.size).is_some_and(|h| h.bounds().expand(reach).contains(p))
            }
            Shape::Pen(points) => match points.as_slice() {
                [] => false,
                [only] => only.distance(p) <= near,
                _ => points.windows(2).any(|w| segment_distance(w[0], w[1], p) <= near),
            },
            Shape::Text { .. } => self.bounds().expand(reach / 2.0).contains(p),
        }
    }
}

/// How far `p` is from the rectangle's outline, inside or out.
fn outline_distance(r: Rect, p: Pos2) -> f32 {
    if r.contains(p) {
        let across = (p.x - r.left()).min(r.right() - p.x);
        let down = (p.y - r.top()).min(r.bottom() - p.y);
        across.min(down)
    } else {
        r.distance_to_pos(p)
    }
}

/// About how far `p` is from the ellipse inside `r`: measured along the
/// line from its centre.
fn ellipse_distance(r: Rect, p: Pos2) -> f32 {
    let (c, radius) = (r.center(), r.size() / 2.0);
    if radius.x <= 0.0 || radius.y <= 0.0 {
        return segment_distance(r.min, r.max, p);
    }
    let d = p - c;
    let scaled = vec2(d.x / radius.x, d.y / radius.y).length();
    if scaled == 0.0 {
        return radius.x.min(radius.y);
    }
    (d.length() * (1.0 - 1.0 / scaled)).abs()
}

/// How far `p` is from the line from `a` to `b`.
fn segment_distance(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    let ab = b - a;
    let length = ab.length_sq();
    if length == 0.0 {
        return a.distance(p);
    }
    let t = ((p - a).dot(ab) / length).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// An arrow's head and where its line stops, under the head.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowHead {
    /// The tip, then the two back corners.
    pub points: [Pos2; 3],
    pub shaft_end: Pos2,
}

impl ArrowHead {
    fn bounds(&self) -> Rect {
        Rect::from_points(&self.points)
    }
}

/// The head of an arrow from `from` to `to` drawn `width` wide: longer for
/// a thicker line, never longer than the arrow. None for an arrow of no
/// length.
pub fn arrow_head(from: Pos2, to: Pos2, width: f32) -> Option<ArrowHead> {
    let along = to - from;
    let length = along.length();
    if length < 0.5 {
        return None;
    }
    let dir = along / length;
    let normal = vec2(-dir.y, dir.x);
    let head = (4.0 * width + 6.0).min(length);
    let base = to - dir * head;
    let half = head * 0.55;
    Some(ArrowHead {
        points: [to, base + normal * half, base - normal * half],
        // Into the head a little, so no gap shows between them.
        shaft_end: to - dir * (head * 0.7),
    })
}

/// A mosaic of `rect` in `image`: blocks of `block` pixels from its
/// top-left corner, each with the average colour of its pixels. The blocks
/// at the right and bottom (and where the picture ends) are cut short.
pub fn mosaic_blocks(image: &Image, rect: Rect, block: f32) -> Vec<(PxRect, Rgba)> {
    let Some(area) = PxRect::clipped(
        rect.min.x.round() as i64,
        rect.min.y.round() as i64,
        rect.max.x.round() as i64,
        rect.max.y.round() as i64,
        image.width,
        image.height,
    ) else {
        return Vec::new();
    };
    // From the rectangle's own corner, wherever the picture cuts it.
    let (x0, y0) = (rect.min.x.round() as i64, rect.min.y.round() as i64);
    let side = block.round().max(2.0) as i64;
    let first = |start: i64, origin: i64| origin + (start - origin).div_euclid(side) * side;
    let (right, bottom) = (i64::from(area.x + area.width), i64::from(area.y + area.height));
    let mut blocks = Vec::new();
    let mut by = first(i64::from(area.y), y0);
    while by < bottom {
        let mut bx = first(i64::from(area.x), x0);
        while bx < right {
            let cell = PxRect::clipped(
                bx.max(i64::from(area.x)),
                by.max(i64::from(area.y)),
                (bx + side).min(right),
                (by + side).min(bottom),
                image.width,
                image.height,
            );
            if let Some(cell) = cell {
                blocks.push((cell, average(image, cell)));
            }
            bx += side;
        }
        by += side;
    }
    blocks
}

/// The average colour of the pixels in `rect`.
fn average(image: &Image, rect: PxRect) -> Rgba {
    let mut sum = [0u64; 4];
    for y in rect.y..rect.y + rect.height {
        let start = (y as usize * image.width as usize + rect.x as usize) * 4;
        for px in image.rgba[start..start + rect.width as usize * 4].as_chunks::<4>().0 {
            for (s, c) in sum.iter_mut().zip(px) {
                *s += u64::from(*c);
            }
        }
    }
    let count = (u64::from(rect.width) * u64::from(rect.height)).max(1);
    sum.map(|s| ((s + count / 2) / count) as u8)
}

/// The marks on a screenshot, in the order drawn, with what undo and redo
/// go back and forth between.
#[derive(Debug, Clone, Default)]
pub struct Marks {
    list: Vec<Mark>,
    /// Earlier lists, the latest last.
    undo: Vec<Vec<Mark>>,
    redo: Vec<Vec<Mark>>,
}

impl Marks {
    pub fn list(&self) -> &[Mark] {
        &self.list
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Keeps the list as it is now for undo; what was undone is gone.
    fn remember(&mut self, list: Vec<Mark>) {
        self.undo.push(list);
        self.redo.clear();
    }

    pub fn add(&mut self, mark: Mark) {
        self.remember(self.list.clone());
        self.list.push(mark);
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.list.len() {
            self.remember(self.list.clone());
            self.list.remove(index);
        }
    }

    /// Changes a mark as it is dragged, which undo does not see until
    /// [`Marks::edited`].
    pub fn set(&mut self, index: usize, mark: Mark) {
        if let Some(slot) = self.list.get_mut(index) {
            *slot = mark;
        }
    }

    /// The mark at `index`, changed with [`Marks::set`], was `before`:
    /// undo takes it back, if it did change.
    pub fn edited(&mut self, index: usize, before: Mark) {
        if self.list.get(index).is_some_and(|now| *now != before) {
            let mut earlier = self.list.clone();
            earlier[index] = before;
            self.remember(earlier);
        }
    }

    /// Back to the list before the last change; false when there was none.
    pub fn undo(&mut self) -> bool {
        let Some(earlier) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.list, earlier));
        true
    }

    /// The change undone last, again.
    pub fn redo(&mut self) -> bool {
        let Some(later) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.list, later));
        true
    }

    /// The top-most mark at `p`, give or take `reach`.
    pub fn at(&self, p: Pos2, reach: f32) -> Option<usize> {
        self.list.iter().rposition(|m| m.hit(p, reach))
    }

    /// Every mark moved by `by`: into a picture cut out at `-by`.
    pub fn translated(&self, by: Vec2) -> Vec<Mark> {
        self.list.iter().map(|m| m.translated(by)).collect()
    }
}

/// A rectangle from `a` to `b` with its corners on whole pixels.
pub fn pixel_rect(a: Pos2, b: Pos2) -> Rect {
    let r = Rect::from_two_pos(a, b);
    Rect::from_min_max(
        pos2(r.min.x.round(), r.min.y.round()),
        pos2(r.max.x.round(), r.max.y.round()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba = [255, 0, 0, 255];

    fn mark(shape: Shape) -> Mark {
        Mark {
            shape,
            color: RED,
            size: 4.0,
        }
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    #[test]
    fn marks_are_hit_on_their_lines() {
        let r = mark(Shape::Rectangle(rect(10.0, 10.0, 110.0, 60.0)));
        assert!(r.hit(pos2(10.0, 30.0), 3.0), "on the left edge");
        assert!(r.hit(pos2(60.0, 64.0), 3.0), "just below");
        assert!(!r.hit(pos2(60.0, 35.0), 3.0), "inside an outline is not on it");
        assert!(!r.hit(pos2(200.0, 35.0), 3.0));

        let e = mark(Shape::Ellipse(rect(0.0, 0.0, 200.0, 100.0)));
        assert!(e.hit(pos2(100.0, 1.0), 2.0), "the top of the curve");
        assert!(e.hit(pos2(199.0, 50.0), 2.0), "its right end");
        assert!(!e.hit(pos2(100.0, 50.0), 2.0), "the middle");
        assert!(!e.hit(pos2(5.0, 5.0), 2.0), "a corner of its box");

        let a = mark(Shape::Arrow {
            from: pos2(0.0, 0.0),
            to: pos2(100.0, 0.0),
        });
        assert!(a.hit(pos2(50.0, 3.0), 2.0));
        assert!(a.hit(pos2(95.0, 9.0), 2.0), "the head is wider than the line");
        assert!(!a.hit(pos2(50.0, 10.0), 2.0));

        let pen = mark(Shape::Pen(vec![pos2(0.0, 0.0), pos2(10.0, 10.0), pos2(20.0, 0.0)]));
        assert!(pen.hit(pos2(15.0, 5.0), 2.0));
        assert!(!pen.hit(pos2(10.0, 0.0), 2.0));
        assert!(mark(Shape::Pen(vec![pos2(5.0, 5.0)])).hit(pos2(6.0, 6.0), 1.0), "a dot");

        let text = mark(Shape::Text {
            at: pos2(10.0, 10.0),
            text: "Hi".into(),
            extent: vec2(30.0, 20.0),
        });
        assert!(text.hit(pos2(20.0, 20.0), 4.0), "anywhere in a text");
        assert!(!text.hit(pos2(50.0, 20.0), 4.0));

        let mosaic = mark(Shape::Mosaic(rect(0.0, 0.0, 100.0, 100.0)));
        assert!(mosaic.hit(pos2(1.0, 50.0), 3.0), "its edge");
        assert!(!mosaic.hit(pos2(50.0, 50.0), 3.0), "inside it, drawing goes on");
    }

    #[test]
    fn moving_and_bounds() {
        let a = mark(Shape::Arrow {
            from: pos2(0.0, 0.0),
            to: pos2(100.0, 0.0),
        });
        let moved = a.translated(vec2(5.0, 7.0));
        assert_eq!(
            moved.shape,
            Shape::Arrow {
                from: pos2(5.0, 7.0),
                to: pos2(105.0, 7.0)
            }
        );
        assert_eq!(moved.color, a.color);
        let b = a.bounds();
        assert!(b.min.y < -10.0 && b.max.y > 10.0, "the head is in the bounds: {b:?}");
        let pen = mark(Shape::Pen(vec![pos2(10.0, 10.0), pos2(20.0, 30.0)]));
        assert_eq!(pen.bounds(), rect(8.0, 8.0, 22.0, 32.0));
        assert!(mark(Shape::Rectangle(rect(0.0, 0.0, 1.0, 50.0))).is_empty());
        assert!(!mark(Shape::Rectangle(rect(0.0, 0.0, 3.0, 50.0))).is_empty());
        let blank = Shape::Text {
            at: Pos2::ZERO,
            text: "  ".into(),
            extent: Vec2::ZERO,
        };
        assert!(mark(blank).is_empty());
    }

    #[test]
    fn arrow_heads_point_at_the_end() {
        let head = arrow_head(pos2(0.0, 0.0), pos2(100.0, 0.0), 2.0).unwrap();
        assert_eq!(head.points[0], pos2(100.0, 0.0));
        // 4 × 2 + 6 = 14 long, 0.55 × 14 to each side.
        assert!(head.points[1].distance(pos2(86.0, 7.7)) < 1e-4, "{:?}", head.points);
        assert!(head.points[2].distance(pos2(86.0, -7.7)) < 1e-4, "{:?}", head.points);
        assert!(head.shaft_end.x > 86.0 && head.shaft_end.x < 100.0);
        // A short arrow is all head.
        let short = arrow_head(pos2(0.0, 0.0), pos2(0.0, 5.0), 8.0).unwrap();
        assert_eq!(short.points[1].y, 0.0);
        assert_eq!(arrow_head(pos2(3.0, 3.0), pos2(3.0, 3.0), 2.0), None);
    }

    #[test]
    fn mosaic_averages_blocks_from_its_corner() {
        // 8 × 4: the left half black, the right half white.
        let rgba = (0..4)
            .flat_map(|_| (0..8).flat_map(|x| if x < 4 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }))
            .collect();
        let image = Image {
            width: 8,
            height: 4,
            rgba,
        };
        let blocks = mosaic_blocks(&image, rect(2.0, 0.0, 9.0, 4.0), 4.0);
        // From x = 2: 2..6 (half black, half white), then 6..8, cut by the picture.
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0].0,
            PxRect {
                x: 2,
                y: 0,
                width: 4,
                height: 4
            }
        );
        assert_eq!(blocks[0].1, [128, 128, 128, 255]);
        assert_eq!(blocks[1].0.width, 2);
        assert_eq!(blocks[1].1, [255, 255, 255, 255]);
        // A rectangle starting off the picture keeps its own grid.
        let off = mosaic_blocks(&image, rect(-3.0, -3.0, 8.0, 4.0), 4.0);
        assert_eq!(off[0].0.width, 1, "the block from -3 covers 0..1");
        assert_eq!(off[0].0.height, 1);
        assert!(mosaic_blocks(&image, rect(20.0, 0.0, 30.0, 4.0), 4.0).is_empty());
    }

    #[test]
    fn undo_and_redo() {
        let a = mark(Shape::Rectangle(rect(0.0, 0.0, 10.0, 10.0)));
        let b = mark(Shape::Ellipse(rect(20.0, 20.0, 40.0, 40.0)));
        let mut marks = Marks::default();
        assert!(!marks.undo());
        marks.add(a.clone());
        marks.add(b.clone());
        assert!(marks.undo());
        assert_eq!(marks.list(), std::slice::from_ref(&a));
        assert!(marks.redo());
        assert_eq!(marks.list(), [a.clone(), b.clone()]);
        assert!(!marks.redo());

        // Moving is one step, and only when it moved.
        let before = b.clone();
        marks.set(1, b.translated(vec2(5.0, 0.0)));
        marks.set(1, b.translated(vec2(9.0, 0.0)));
        marks.edited(1, before.clone());
        marks.set(0, a.clone());
        marks.edited(0, a.clone());
        assert!(marks.undo());
        assert_eq!(marks.list()[1], before);
        assert!(marks.undo());
        assert_eq!(marks.list(), std::slice::from_ref(&a));

        // A new change drops what was undone.
        marks.remove(0);
        assert!(marks.is_empty());
        assert!(!marks.can_redo());
        assert!(marks.undo());
        assert_eq!(marks.list(), [a]);
    }

    #[test]
    fn the_top_mark_is_found_first() {
        let mut marks = Marks::default();
        marks.add(mark(Shape::Rectangle(rect(0.0, 0.0, 100.0, 100.0))));
        marks.add(mark(Shape::Arrow {
            from: pos2(0.0, 50.0),
            to: pos2(100.0, 50.0),
        }));
        assert_eq!(marks.at(pos2(0.0, 50.0), 2.0), Some(1));
        assert_eq!(marks.at(pos2(50.0, 0.0), 2.0), Some(0));
        assert_eq!(marks.at(pos2(50.0, 25.0), 2.0), None);
        let cut = marks.translated(vec2(-10.0, -10.0));
        assert_eq!(cut[0].shape, Shape::Rectangle(rect(-10.0, -10.0, 90.0, 90.0)));
        assert_eq!(pixel_rect(pos2(5.6, 9.2), pos2(1.2, 3.5)), rect(1.0, 4.0, 6.0, 9.0));
    }
}
