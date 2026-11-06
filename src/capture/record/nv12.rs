//! A screen recording's pictures on Windows, turned from the screen's BGRA
//! pixels into NV12, which H.264 encoders take: the brightness of every
//! pixel, then the colour of each 2×2 square, in BT.709's limited range
//! (16–235), as HD video is.

/// `bgra` (rows from the top, `stride` bytes apart) as NV12. `width` and
/// `height` are even: a full plane of brightness, then a half-size plane
/// of colour, U and V in turn.
pub fn from_bgra(bgra: &[u8], stride: usize, width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 3 / 2];
    let (luma, chroma) = out.split_at_mut(width * height);
    for y in 0..height {
        let row = &bgra[y * stride..y * stride + width * 4];
        for (x, &[b, g, r, _]) in row.as_chunks::<4>().0.iter().enumerate() {
            let [b, g, r] = [b, g, r].map(i32::from);
            luma[y * width + x] = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
        }
    }
    for y in 0..height / 2 {
        for x in 0..width / 2 {
            // The square's average.
            let mut sum = [0i32; 3];
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let at = (2 * y + dy) * stride + (2 * x + dx) * 4;
                for (c, total) in sum.iter_mut().enumerate() {
                    *total += i32::from(bgra[at + c]);
                }
            }
            let [b, g, r] = sum.map(|v| (v + 2) / 4);
            let at = y * width + 2 * x;
            chroma[at] = (((-26 * r - 87 * g + 113 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
            chroma[at + 1] = (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` × `height` picture of one colour, with `pad` spare bytes after each row.
    fn filled(width: usize, height: usize, pad: usize, [r, g, b]: [u8; 3]) -> Vec<u8> {
        let mut row: Vec<u8> = (0..width).flat_map(|_| [b, g, r, 255]).collect();
        row.extend(std::iter::repeat_n(0, pad));
        row.repeat(height)
    }

    #[test]
    fn colours_land_in_the_limited_range() {
        for (rgb, want) in [
            ([255, 255, 255], [235, 128, 128]),
            ([0, 0, 0], [16, 128, 128]),
            ([255, 0, 0], [63, 102, 240]),
            ([0, 0, 255], [32, 240, 118]),
            ([128, 128, 128], [126, 128, 128]),
        ] {
            let nv12 = from_bgra(&filled(4, 2, 8, rgb), 4 * 4 + 8, 4, 2);
            assert_eq!(nv12.len(), 12);
            assert!(nv12[..8].iter().all(|&y| y == want[0]), "{rgb:?}: {nv12:?}");
            assert_eq!(&nv12[8..10], &want[1..], "{rgb:?}");
            assert_eq!(nv12[8..10], nv12[10..12]);
        }
    }

    #[test]
    fn each_square_gets_its_own_colour() {
        // Left half black, right half white: the brightness follows each
        // pixel, the colour each 2×2 square.
        let mut bgra = filled(4, 2, 0, [0, 0, 0]);
        for y in 0..2 {
            for x in 2..4 {
                bgra[y * 16 + x * 4..y * 16 + x * 4 + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let nv12 = from_bgra(&bgra, 16, 4, 2);
        assert_eq!(&nv12[..4], &[16, 16, 235, 235]);
        assert_eq!(&nv12[8..], &[128, 128, 128, 128]);
    }
}
