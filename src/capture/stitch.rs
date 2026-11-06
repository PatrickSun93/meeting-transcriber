//! Joining the pictures of a scrolling screenshot into one long picture.
//! Each picture is boiled down to a few grey blocks per pixel row
//! ([`Rows`]); comparing two pictures' rows says how far the content moved
//! up between them ([`motion`]), leaving out a header or footer that stays
//! put and letting small changes pass (a blinking cursor, a scrollbar,
//! noise). [`Stitcher`] adds the rows that came into view.

use super::image::Image;

/// At most this many blocks across a row.
const BLOCKS: usize = 128;
/// Rows whose blocks differ by at most this much on average (in grey
/// levels, 0–255) are the same row.
const SAME: u32 = 6;
/// The most one row adds to a comparison, so a few changed rows do not
/// outweigh all the others.
const CAP: u32 = 48;
/// At most this share of the rows compared may differ: after a move, or
/// for pictures to count as the same.
const MAX_CHANGED: f64 = 0.15;
/// A header or footer that stays put is at most a third of the picture.
const FIXED_SHARE: usize = 3;
/// After a move, at least a sixth of the rows that move (and at least
/// `MIN_OVERLAP`) must still be in both pictures to compare.
const OVERLAP_SHARE: usize = 6;
const MIN_OVERLAP: usize = 4;

/// A picture boiled down for comparing: each pixel row as `blocks` grey
/// levels, the averages of the pixels across.
pub struct Rows {
    blocks: usize,
    grey: Vec<u8>,
}

impl Rows {
    pub fn of(image: &Image) -> Rows {
        let (width, height) = (image.width as usize, image.height as usize);
        if width == 0 {
            return Rows {
                blocks: 1,
                grey: Vec::new(),
            };
        }
        let blocks = width.min(BLOCKS);
        let mut grey = Vec::with_capacity(blocks * height);
        for row in image.rgba.chunks_exact(width * 4).take(height) {
            for b in 0..blocks {
                let (from, to) = (
                    b * width / blocks,
                    ((b + 1) * width / blocks).max(b * width / blocks + 1),
                );
                let sum: u32 = row[from * 4..to * 4]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|[r, g, b, _]| (77 * u32::from(*r) + 150 * u32::from(*g) + 29 * u32::from(*b)) >> 8)
                    .sum();
                grey.push((sum / (to - from) as u32) as u8);
            }
        }
        Rows { blocks, grey }
    }

    fn height(&self) -> usize {
        self.grey.len() / self.blocks
    }

    fn row(&self, y: usize) -> &[u8] {
        &self.grey[y * self.blocks..(y + 1) * self.blocks]
    }
}

/// How much two rows differ: the sum over their blocks.
fn diff(a: &[u8], b: &[u8]) -> u32 {
    a.iter().zip(b).map(|(a, b)| u32::from(a.abs_diff(*b))).sum()
}

/// How the content moved from one picture to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// It did not (or hardly changed where it is).
    Still,
    /// The rows from `top` to `bottom` moved up `by` rows; those above and
    /// below them stayed put (a sticky header, a footer).
    Scrolled { top: usize, bottom: usize, by: usize },
    /// No way to tell: it moved too far, back up, or changed.
    Lost,
}

/// How the content moved up from `before` to `after`, pictures of the same
/// place a moment apart.
pub fn motion(before: &Rows, after: &Rows) -> Motion {
    let height = before.height();
    if height != after.height() || before.blocks != after.blocks || height == 0 {
        return Motion::Lost;
    }
    let same_at = SAME * before.blocks as u32;
    let same = |y: usize| diff(before.row(y), after.row(y)) <= same_at;
    let top = (0..height).take_while(|&y| same(y)).count();
    if top == height {
        return Motion::Still;
    }
    let bottom = height - (0..height).rev().take_while(|&y| same(y)).count();
    let changed = (top..bottom).filter(|&y| !same(y)).count();
    // Rows that happen to be alike (blank ones) are not taken for a header.
    let top = top.min(height / FIXED_SHARE);
    let bottom = bottom.max(height - height / FIXED_SHARE);
    let band = bottom - top;
    let min_overlap = (band / OVERLAP_SHARE).max(MIN_OVERLAP);
    let compare = Compare {
        before,
        after,
        top,
        bottom,
        same_at,
    };
    if band > min_overlap {
        let still = compare.cost(0, f64::INFINITY).unwrap_or(f64::INFINITY);
        let mut best: Option<(f64, usize)> = None;
        for by in compare.candidates(band - min_overlap) {
            let limit = best.map_or(f64::INFINITY, |(cost, _)| cost);
            if let Some(cost) = compare.cost(by, limit)
                && best.is_none_or(|(c, d)| cost < c || (cost == c && by < d))
            {
                best = Some((cost, by));
            }
        }
        if let Some((cost, by)) = best
            && cost < still
            && compare.changed(by) <= MAX_CHANGED
        {
            return Motion::Scrolled { top, bottom, by };
        }
    }
    if changed as f64 <= MAX_CHANGED * height as f64 {
        Motion::Still
    } else {
        Motion::Lost
    }
}

/// Two pictures' rows between `top` and `bottom`, compared at offsets.
struct Compare<'a> {
    before: &'a Rows,
    after: &'a Rows,
    top: usize,
    bottom: usize,
    same_at: u32,
}

impl Compare<'_> {
    /// The pairs of rows that would be the same if the content moved up
    /// `by`: `after`'s row and `before`'s row `by` lower.
    fn pairs(&self, by: usize) -> impl Iterator<Item = (&[u8], &[u8])> {
        (self.top..self.bottom - by).map(move |y| (self.after.row(y), self.before.row(y + by)))
    }

    /// How unlike the rows are if the content moved up `by`: the average
    /// grey difference per block, each row's capped. None once it is sure
    /// to come out above `limit`.
    fn cost(&self, by: usize, limit: f64) -> Option<f64> {
        let blocks = self.before.blocks as f64;
        let rows = (self.bottom - self.top - by) as f64;
        let most = limit * rows * blocks;
        let cap = CAP * self.before.blocks as u32;
        let mut total = 0u64;
        for (a, b) in self.pairs(by) {
            total += u64::from(diff(a, b).min(cap));
            if total as f64 > most {
                return None;
            }
        }
        Some(total as f64 / (rows * blocks))
    }

    /// The share of the rows that differ if the content moved up `by`.
    fn changed(&self, by: usize) -> f64 {
        let rows = self.bottom - self.top - by;
        let changed = self.pairs(by).filter(|(a, b)| diff(a, b) > self.same_at).count();
        changed as f64 / rows.max(1) as f64
    }

    /// The offsets from 1 to `most`, the likeliest first: by how well the
    /// rows' overall brightness lines up, which is quick to check, so the
    /// full comparison of the right one comes early and cuts the others short.
    fn candidates(&self, most: usize) -> Vec<usize> {
        let brightness = |rows: &Rows| -> Vec<u32> {
            (self.top..self.bottom)
                .map(|y| rows.row(y).iter().map(|g| u32::from(*g)).sum())
                .collect()
        };
        let (before, after) = (brightness(self.before), brightness(self.after));
        let mut order: Vec<(u64, usize)> = (1..=most)
            .map(|by| {
                let rows = after.len() - by;
                let total: u64 = (0..rows).map(|i| u64::from(after[i].abs_diff(before[i + by]))).sum();
                (total * 1024 / rows as u64, by)
            })
            .collect();
        order.sort_unstable();
        order.into_iter().map(|(_, by)| by).collect()
    }
}

/// What one more picture did to the long one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// It grew by this many rows.
    Grew(u32),
    /// Nothing moved.
    Still,
    /// The picture did not fit on (it moved too far, back, or changed);
    /// the next is compared with the last that did.
    Lost,
}

/// The long picture, grown one picture of the scrolling area at a time.
pub struct Stitcher {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// The last picture that fitted on: its height and its rows.
    frame_height: u32,
    rows: Rows,
    /// How many rows at the end of the long picture are the last picture's
    /// bottom rows: those it can still change.
    tail: u32,
}

impl Stitcher {
    pub fn new(first: Image) -> Stitcher {
        Stitcher {
            width: first.width,
            height: first.height,
            frame_height: first.height,
            rows: Rows::of(&first),
            tail: first.height,
            rgba: first.rgba,
        }
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Adds the next picture of the area: the rows that came into view at
    /// the bottom go in above the footer.
    pub fn push(&mut self, frame: Image) -> Step {
        if frame.width != self.width || frame.height != self.frame_height {
            return Step::Lost;
        }
        let rows = Rows::of(&frame);
        let row = self.width as usize * 4;
        let height = frame.height as usize;
        let step = match motion(&self.rows, &rows) {
            Motion::Lost => return Step::Lost,
            Motion::Still => {
                // What changed in place (a picture that loaded) is kept.
                let tail = self.tail as usize;
                let start = (self.height as usize - tail) * row;
                self.rgba[start..].copy_from_slice(&frame.rgba[(height - tail) * row..height * row]);
                Step::Still
            }
            Motion::Scrolled { top, bottom, by } => {
                // The footer at the end goes; the new rows and it come back.
                let kept = self.height as usize - (height - bottom);
                self.rgba.truncate(kept * row);
                self.rgba
                    .extend_from_slice(&frame.rgba[(bottom - by) * row..height * row]);
                self.height = (kept + height - bottom + by) as u32;
                self.tail = (height - top) as u32;
                Step::Grew(by as u32)
            }
        };
        self.rows = rows;
        step
    }

    /// The long picture, cut at `max_height` rows.
    pub fn finish(mut self, max_height: u32) -> Image {
        let height = self.height.min(max_height);
        self.rgba.truncate(height as usize * self.width as usize * 4);
        Image {
            width: self.width,
            height,
            rgba: self.rgba,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::select::PxRect;
    use super::*;

    /// Numbers that look random, the same each run.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A page like text: lines of dark dashes on white, a few blank rows
    /// between them, every row of a line its own pattern.
    fn page(width: u32, height: u32, seed: u64) -> Image {
        let mut random = Random(seed);
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        let white = [255u8, 255, 255, 255];
        let mut y = 0;
        while y < height {
            let (line, gap) = (8 + random.below(8) as u32, 2 + random.below(8) as u32);
            for _ in 0..line.min(height - y) {
                let mut x = 0;
                while x < width {
                    let (dash, space) = (3 + random.below(18) as u32, 1 + random.below(6) as u32);
                    let ink = [
                        random.below(120) as u8,
                        random.below(120) as u8,
                        random.below(200) as u8,
                        255,
                    ];
                    for i in 0..(dash + space).min(width - x) {
                        rgba.extend_from_slice(if i < dash { &ink } else { &white });
                    }
                    x += dash + space;
                }
            }
            for _ in 0..gap.min(height - y - line.min(height - y)) {
                for _ in 0..width {
                    rgba.extend_from_slice(&white);
                }
            }
            y += line + gap;
        }
        Image { width, height, rgba }
    }

    fn rows(image: &Image, top: u32, height: u32) -> Image {
        image.crop(PxRect {
            x: 0,
            y: top,
            width: image.width,
            height,
        })
    }

    /// Pictures one above the other.
    fn stack(parts: &[&Image]) -> Image {
        Image {
            width: parts[0].width,
            height: parts.iter().map(|p| p.height).sum(),
            rgba: parts.iter().flat_map(|p| p.rgba.iter().copied()).collect(),
        }
    }

    /// Every channel moved by up to `by` either way.
    fn noisy(image: &Image, by: u8, seed: u64) -> Image {
        let mut random = Random(seed);
        let rgba = image
            .rgba
            .iter()
            .map(|&c| {
                let n = random.below(u64::from(by) * 2 + 1) as i16 - i16::from(by);
                (i16::from(c) + n).clamp(0, 255) as u8
            })
            .collect();
        Image { rgba, ..image.clone() }
    }

    /// Scrolls through `content` with a fixed header and footer around a
    /// window of it, stopping at each of `tops`; returns the steps and
    /// the long picture.
    fn scroll(content: &Image, header: &Image, footer: &Image, window: u32, tops: &[u32]) -> (Vec<Step>, Image) {
        let frame = |top| stack(&[header, &rows(content, top, window), footer]);
        let mut stitcher = Stitcher::new(frame(tops[0]));
        let steps = tops[1..].iter().map(|&top| stitcher.push(frame(top))).collect();
        (steps, stitcher.finish(u32::MAX))
    }

    #[test]
    fn a_scrolled_page_comes_out_whole() {
        let content = page(160, 1500, 7);
        let none = Image {
            width: 160,
            height: 0,
            rgba: Vec::new(),
        };
        let tops = [0, 120, 250, 250, 431, 600, 820, 1000, 1150, 1200];
        let (steps, long) = scroll(&content, &none, &none, 300, &tops);
        let moves: Vec<Step> = tops
            .windows(2)
            .map(|t| match t[1] - t[0] {
                0 => Step::Still,
                by => Step::Grew(by),
            })
            .collect();
        assert_eq!(steps, moves);
        assert_eq!(long, content, "every row once, in order");
    }

    #[test]
    fn a_sticky_header_and_footer_show_once() {
        let content = page(200, 1200, 11);
        let header = rows(&page(200, 60, 3), 0, 40);
        let footer = rows(&page(200, 60, 5), 0, 24);
        let tops = [0, 90, 180, 333, 500, 690, 800, 964];
        let (steps, long) = scroll(&content, &header, &footer, 236, &tops);
        assert!(steps.iter().all(|s| matches!(s, Step::Grew(_))), "{steps:?}");
        assert_eq!(long, stack(&[&header, &content, &footer]));
    }

    #[test]
    fn noise_does_not_throw_it_off() {
        let content = page(180, 1000, 13);
        let tops = [0u32, 100, 230, 230, 400, 555, 700];
        let window = 300;
        let mut stitcher = Stitcher::new(noisy(&rows(&content, 0, window), 4, 1));
        for (i, &top) in tops[1..].iter().enumerate() {
            stitcher.push(noisy(&rows(&content, top, window), 4, i as u64 + 2));
        }
        let long = stitcher.finish(u32::MAX);
        assert_eq!((long.width, long.height), (180, 1000));
        let worst = long.rgba.iter().zip(&content.rgba).map(|(a, b)| a.abs_diff(*b)).max();
        assert!(worst <= Some(4), "{worst:?}");
    }

    #[test]
    fn too_far_back_or_another_size_is_lost_and_skipped() {
        let content = page(120, 900, 17);
        let window = 240;
        let mut stitcher = Stitcher::new(rows(&content, 0, window));
        // Moved by more than the pictures overlap.
        assert_eq!(stitcher.push(rows(&content, 235, window)), Step::Lost);
        assert_eq!(stitcher.height(), window);
        // The next one is compared with the last that fitted.
        assert_eq!(stitcher.push(rows(&content, 100, window)), Step::Grew(100));
        // Back up: lost, nothing added.
        assert_eq!(stitcher.push(rows(&content, 40, window)), Step::Lost);
        assert_eq!(stitcher.push(rows(&content, 100, 200)), Step::Lost);
        assert_eq!(stitcher.push(rows(&content, 100, window)), Step::Still);
        assert_eq!(stitcher.push(rows(&content, 260, window)), Step::Grew(160));
        assert_eq!(stitcher.finish(u32::MAX), rows(&content, 0, 500));
    }

    #[test]
    fn small_changes_in_place_are_still_and_kept() {
        let content = page(150, 300, 19);
        let mut stitcher = Stitcher::new(content.clone());
        // A blinking cursor: two pixels wide, a line tall.
        let mut cursor = content.clone();
        for y in 100..114 {
            for x in 60..62 {
                let i = (y * 150 + x) * 4;
                cursor.rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        assert_eq!(stitcher.push(cursor.clone()), Step::Still);
        assert_eq!(stitcher.push(content.clone()), Step::Still);
        // A picture loading in place over a few rows: kept.
        let mut loaded = content.clone();
        for i in (150 * 4 * 200)..(150 * 4 * 230) {
            loaded.rgba[i] = 90;
        }
        assert_eq!(stitcher.push(loaded.clone()), Step::Still);
        assert_eq!(stitcher.finish(u32::MAX), loaded);
    }

    #[test]
    fn motion_finds_the_band_that_moves() {
        let content = page(100, 600, 23);
        let header = rows(&page(100, 50, 29), 0, 30);
        let a = stack(&[&header, &rows(&content, 0, 200)]);
        let b = stack(&[&header, &rows(&content, 70, 200)]);
        match motion(&Rows::of(&a), &Rows::of(&b)) {
            Motion::Scrolled { top, bottom, by } => {
                assert!((30..=230 / 3).contains(&top), "{top}");
                assert!(bottom >= 230 - 230 / 3, "{bottom}");
                assert_eq!(by, 70);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(motion(&Rows::of(&a), &Rows::of(&a)), Motion::Still);
        let other = page(100, 230, 31);
        assert_eq!(motion(&Rows::of(&a), &Rows::of(&other)), Motion::Lost);
    }

    #[test]
    fn finishing_cuts_at_the_limit() {
        let content = page(64, 400, 37);
        let mut stitcher = Stitcher::new(rows(&content, 0, 200));
        assert_eq!(stitcher.push(rows(&content, 150, 200)), Step::Grew(150));
        assert_eq!(stitcher.height(), 350);
        let cut = stitcher.finish(300);
        assert_eq!(cut, rows(&content, 0, 300));
    }
}
