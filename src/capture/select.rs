//! The selection on the screenshot overlay: points on the overlay to the
//! screenshot's pixels and back, drawing a rectangle, its handles and
//! dragging them, keeping it on the display, and where the toolbar goes.

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

/// A rectangle of whole pixels in a picture, from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PxRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PxRect {
    /// The part of the rectangle from (`x0`, `y0`) to (`x1`, `y1`) inside a
    /// picture of `width` × `height`; None when nothing of it is.
    pub fn clipped(x0: i64, y0: i64, x1: i64, y1: i64, width: u32, height: u32) -> Option<PxRect> {
        let (x0, x1) = (x0.clamp(0, i64::from(width)), x1.clamp(0, i64::from(width)));
        let (y0, y1) = (y0.clamp(0, i64::from(height)), y1.clamp(0, i64::from(height)));
        (x1 > x0 && y1 > y0).then(|| PxRect {
            x: x0 as u32,
            y: y0 as u32,
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
        })
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32
            && y >= self.y as f32
            && x < (self.x + self.width) as f32
            && y < (self.y + self.height) as f32
    }
}

/// How the overlay's points lie over the screenshot's pixels: `screen` (in
/// points) shows the whole picture of `width` × `height` pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    pub screen: Rect,
    pub width: u32,
    pub height: u32,
}

impl Mapping {
    /// Pixels per point, across and down.
    fn scale(&self) -> Vec2 {
        vec2(
            self.width as f32 / self.screen.width().max(1.0),
            self.height as f32 / self.screen.height().max(1.0),
        )
    }

    /// The pixel position under a point.
    pub fn to_pixel(&self, point: Pos2) -> Pos2 {
        let s = self.scale();
        let p = point - self.screen.min;
        pos2(p.x * s.x, p.y * s.y)
    }

    /// The pixels a rectangle of points covers, rounded to whole pixels and
    /// kept inside the picture; None when that leaves nothing.
    pub fn to_pixels(&self, rect: Rect) -> Option<PxRect> {
        let (a, b) = (self.to_pixel(rect.min), self.to_pixel(rect.max));
        PxRect::clipped(
            a.x.round() as i64,
            a.y.round() as i64,
            b.x.round() as i64,
            b.y.round() as i64,
            self.width,
            self.height,
        )
    }

    /// Where pixels lie on the overlay.
    pub fn to_points(&self, rect: PxRect) -> Rect {
        let s = self.scale();
        let min = self.screen.min + vec2(rect.x as f32 / s.x, rect.y as f32 / s.y);
        Rect::from_min_size(min, vec2(rect.width as f32 / s.x, rect.height as f32 / s.y))
    }
}

/// What dragging a part of the selection changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// Inside: moves the whole selection.
    Move,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Handle {
    /// The handles drawn on the selection, corners first.
    pub const DRAWN: [Handle; 8] = [
        Handle::TopLeft,
        Handle::TopRight,
        Handle::BottomLeft,
        Handle::BottomRight,
        Handle::Top,
        Handle::Bottom,
        Handle::Left,
        Handle::Right,
    ];

    /// Where the handle sits on `rect`.
    pub fn position(self, rect: Rect) -> Pos2 {
        let c = rect.center();
        match self {
            Handle::Move => c,
            Handle::Left => pos2(rect.left(), c.y),
            Handle::Right => pos2(rect.right(), c.y),
            Handle::Top => pos2(c.x, rect.top()),
            Handle::Bottom => pos2(c.x, rect.bottom()),
            Handle::TopLeft => rect.left_top(),
            Handle::TopRight => rect.right_top(),
            Handle::BottomLeft => rect.left_bottom(),
            Handle::BottomRight => rect.right_bottom(),
        }
    }

    /// Which edges it moves: left, right, top, bottom.
    fn edges(self) -> [bool; 4] {
        match self {
            Handle::Move => [true; 4],
            Handle::Left => [true, false, false, false],
            Handle::Right => [false, true, false, false],
            Handle::Top => [false, false, true, false],
            Handle::Bottom => [false, false, false, true],
            Handle::TopLeft => [true, false, true, false],
            Handle::TopRight => [false, true, true, false],
            Handle::BottomLeft => [true, false, false, true],
            Handle::BottomRight => [false, true, false, true],
        }
    }
}

/// The handle under `point`, within `reach` points of it: a corner or an
/// edge, else inside the selection to move it; None outside.
pub fn handle_at(selection: Rect, point: Pos2, reach: f32) -> Option<Handle> {
    let near = |h: Handle| h.position(selection).distance(point) <= reach;
    if let Some(corner) = Handle::DRAWN[..4].iter().copied().find(|h| near(*h)) {
        return Some(corner);
    }
    let grown = selection.expand(reach);
    if !grown.contains(point) {
        return None;
    }
    let (dx_left, dx_right) = ((point.x - selection.left()).abs(), (point.x - selection.right()).abs());
    let (dy_top, dy_bottom) = ((point.y - selection.top()).abs(), (point.y - selection.bottom()).abs());
    let edge = [
        (dx_left, Handle::Left),
        (dx_right, Handle::Right),
        (dy_top, Handle::Top),
        (dy_bottom, Handle::Bottom),
    ]
    .into_iter()
    .filter(|(d, _)| *d <= reach)
    .min_by(|a, b| a.0.total_cmp(&b.0));
    match edge {
        Some((_, handle)) => Some(handle),
        None if selection.contains(point) => Some(Handle::Move),
        None => None,
    }
}

/// The rectangle between two points, kept inside `bounds`.
pub fn span(a: Pos2, b: Pos2, bounds: Rect) -> Rect {
    Rect::from_two_pos(bounds.clamp(a), bounds.clamp(b))
}

/// `start` with `handle` dragged by `delta`, inside `bounds`. An edge
/// dragged past the opposite one turns the selection over; moved, the
/// selection keeps its size and stops at the edges of `bounds`.
pub fn drag(start: Rect, handle: Handle, delta: Vec2, bounds: Rect) -> Rect {
    if handle == Handle::Move {
        let min = start.min + delta;
        let max_x = (bounds.right() - start.width()).max(bounds.left());
        let max_y = (bounds.bottom() - start.height()).max(bounds.top());
        let min = pos2(min.x.clamp(bounds.left(), max_x), min.y.clamp(bounds.top(), max_y));
        return Rect::from_min_size(min, start.size()).intersect(bounds);
    }
    let [left, right, top, bottom] = handle.edges();
    let x0 = start.left() + if left { delta.x } else { 0.0 };
    let x1 = start.right() + if right { delta.x } else { 0.0 };
    let y0 = start.top() + if top { delta.y } else { 0.0 };
    let y1 = start.bottom() + if bottom { delta.y } else { 0.0 };
    span(pos2(x0, y0), pos2(x1, y1), bounds)
}

/// Where a toolbar of `size` goes: under the selection's right end, else
/// above it, else inside its bottom right; always within `bounds`.
pub fn toolbar_origin(selection: Rect, size: Vec2, bounds: Rect, gap: f32) -> Pos2 {
    let x = (selection.right() - size.x).clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left()));
    let y = if selection.bottom() + gap + size.y <= bounds.bottom() {
        selection.bottom() + gap
    } else if selection.top() - gap - size.y >= bounds.top() {
        selection.top() - gap - size.y
    } else {
        (selection.bottom() - gap - size.y).max(bounds.top())
    };
    pos2(x, y)
}

/// The front-most window under the pixel (`x`, `y`); `windows` are listed
/// front to back.
pub fn window_at(windows: &[PxRect], x: f32, y: f32) -> Option<PxRect> {
    windows.iter().copied().find(|w| w.contains(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1440 × 900 point display shown at 2× (a Retina Mac).
    fn retina() -> Mapping {
        Mapping {
            screen: Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0)),
            width: 2880,
            height: 1800,
        }
    }

    #[test]
    fn points_and_pixels() {
        let m = retina();
        assert_eq!(m.to_pixel(pos2(10.0, 20.5)), pos2(20.0, 41.0));
        let rect = Rect::from_min_max(pos2(10.0, 20.0), pos2(110.25, 70.0));
        let px = m.to_pixels(rect).unwrap();
        assert_eq!(
            px,
            PxRect {
                x: 20,
                y: 40,
                width: 201,
                height: 100
            }
        );
        assert_eq!(
            m.to_points(PxRect {
                x: 20,
                y: 40,
                width: 200,
                height: 100
            }),
            Rect::from_min_max(pos2(10.0, 20.0), pos2(110.0, 70.0))
        );
        // Past the edges is cut off; outside entirely is nothing.
        let past = Rect::from_min_max(pos2(-50.0, 850.0), pos2(30.0, 950.0));
        assert_eq!(
            m.to_pixels(past),
            Some(PxRect {
                x: 0,
                y: 1700,
                width: 60,
                height: 100
            })
        );
        assert_eq!(m.to_pixels(Rect::from_min_max(pos2(-9.0, 0.0), pos2(-1.0, 5.0))), None);
        // A screen that does not start at 0 (an overlay's rect in its window).
        let offset = Mapping {
            screen: Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0)),
            width: 800,
            height: 600,
        };
        assert_eq!(offset.to_pixel(pos2(100.0, 50.0)), Pos2::ZERO);
        assert_eq!(PxRect::clipped(-5, 3, 4, 2, 10, 10), None, "upside down is nothing");
    }

    #[test]
    fn handles_are_found_corners_first() {
        let sel = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        assert_eq!(handle_at(sel, pos2(103.0, 98.0), 8.0), Some(Handle::TopLeft));
        assert_eq!(handle_at(sel, pos2(300.0, 205.0), 8.0), Some(Handle::BottomRight));
        assert_eq!(handle_at(sel, pos2(200.0, 104.0), 8.0), Some(Handle::Top));
        assert_eq!(handle_at(sel, pos2(295.0, 150.0), 8.0), Some(Handle::Right));
        assert_eq!(handle_at(sel, pos2(150.0, 150.0), 8.0), Some(Handle::Move));
        assert_eq!(handle_at(sel, pos2(50.0, 150.0), 8.0), None);
        assert_eq!(Handle::Bottom.position(sel), pos2(200.0, 200.0));
    }

    #[test]
    fn dragging_resizes_moves_and_stays_inside() {
        let bounds = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
        let sel = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        assert_eq!(
            drag(sel, Handle::BottomRight, vec2(50.0, 20.0), bounds),
            Rect::from_min_max(pos2(100.0, 100.0), pos2(350.0, 220.0))
        );
        assert_eq!(
            drag(sel, Handle::Left, vec2(-30.0, 999.0), bounds),
            Rect::from_min_max(pos2(70.0, 100.0), pos2(300.0, 200.0))
        );
        // Past the opposite edge: turned over.
        assert_eq!(
            drag(sel, Handle::Top, vec2(0.0, 150.0), bounds),
            Rect::from_min_max(pos2(100.0, 200.0), pos2(300.0, 250.0))
        );
        // Out of the display: stopped at its edge.
        assert_eq!(
            drag(sel, Handle::TopLeft, vec2(-500.0, -500.0), bounds),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(300.0, 200.0))
        );
        // Moved: same size, stops at the edges.
        assert_eq!(
            drag(sel, Handle::Move, vec2(900.0, -300.0), bounds),
            Rect::from_min_max(pos2(800.0, 0.0), pos2(1000.0, 100.0))
        );
        assert_eq!(
            span(pos2(500.0, 900.0), pos2(-5.0, 10.0), bounds),
            Rect::from_min_max(pos2(0.0, 10.0), pos2(500.0, 800.0))
        );
    }

    #[test]
    fn toolbar_goes_below_else_above_else_inside() {
        let bounds = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0));
        let size = vec2(200.0, 40.0);
        let sel = Rect::from_min_max(pos2(100.0, 100.0), pos2(400.0, 300.0));
        assert_eq!(toolbar_origin(sel, size, bounds, 8.0), pos2(200.0, 308.0));
        let low = Rect::from_min_max(pos2(100.0, 500.0), pos2(400.0, 790.0));
        assert_eq!(toolbar_origin(low, size, bounds, 8.0), pos2(200.0, 452.0));
        let full = bounds;
        assert_eq!(toolbar_origin(full, size, bounds, 8.0), pos2(800.0, 752.0));
        // A narrow selection at the left edge: kept on the display.
        let narrow = Rect::from_min_max(pos2(0.0, 100.0), pos2(50.0, 150.0));
        assert_eq!(toolbar_origin(narrow, size, bounds, 8.0).x, 0.0);
    }

    #[test]
    fn the_front_window_wins() {
        let back = PxRect {
            x: 0,
            y: 0,
            width: 500,
            height: 500,
        };
        let front = PxRect {
            x: 100,
            y: 100,
            width: 50,
            height: 50,
        };
        assert_eq!(window_at(&[front, back], 120.0, 120.0), Some(front));
        assert_eq!(window_at(&[front, back], 10.0, 10.0), Some(back));
        assert_eq!(window_at(&[front, back], 600.0, 10.0), None);
        assert!(!front.contains(150.0, 120.0));
    }
}
