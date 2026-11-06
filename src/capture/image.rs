//! Pictures in memory: RGBA pixels as the systems' screen grabs come in,
//! cropped to a selection, shrunk for a thumbnail, and PNG files.

use anyhow::{Context as _, Result, bail};

use super::select::PxRect;

/// An RGBA picture: rows from the top, four bytes a pixel, straight alpha.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}×{})", self.width, self.height)
    }
}

/// How 32-bit pixels are packed: the bits of each colour (the rest is
/// unused or alpha), read as a little- or big-endian number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packing {
    pub red: u32,
    pub green: u32,
    pub blue: u32,
    pub big_endian: bool,
}

impl Packing {
    /// Blue, green, red, unused in memory: GDI's DIBs, Quartz's
    /// `kCGImageByteOrder32Little` with the alpha first, most X servers.
    pub const BGRX: Packing = Packing {
        red: 0x00ff_0000,
        green: 0x0000_ff00,
        blue: 0x0000_00ff,
        big_endian: false,
    };
}

/// The 8-bit value of the colour in `mask`, from a pixel.
fn channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shifted = (pixel & mask) >> mask.trailing_zeros();
    let max = mask >> mask.trailing_zeros();
    if max == 0xff {
        shifted as u8
    } else {
        (shifted * 255 / max) as u8
    }
}

impl Image {
    /// From 32-bit pixels packed as `packing`, `stride` bytes a row. Every
    /// pixel comes out opaque: screens have no transparency.
    pub fn from_packed(width: u32, height: u32, stride: usize, data: &[u8], packing: Packing) -> Result<Image> {
        let row = width as usize * 4;
        if stride < row || data.len() < stride * (height as usize).saturating_sub(1) + row {
            bail!(
                "the screen's pixels came in short ({} bytes for {width}×{height})",
                data.len()
            );
        }
        let mut rgba = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            for bytes in data[y * stride..y * stride + row].as_chunks::<4>().0 {
                let pixel = if packing.big_endian {
                    u32::from_be_bytes(*bytes)
                } else {
                    u32::from_le_bytes(*bytes)
                };
                rgba.extend_from_slice(&[
                    channel(pixel, packing.red),
                    channel(pixel, packing.green),
                    channel(pixel, packing.blue),
                    255,
                ]);
            }
        }
        Ok(Image { width, height, rgba })
    }

    /// The part inside `rect`, which must lie within the picture.
    pub fn crop(&self, rect: PxRect) -> Image {
        let (x, y) = (rect.x.min(self.width), rect.y.min(self.height));
        let width = rect.width.min(self.width - x);
        let height = rect.height.min(self.height - y);
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for row in y..y + height {
            let start = (row as usize * self.width as usize + x as usize) * 4;
            rgba.extend_from_slice(&self.rgba[start..start + width as usize * 4]);
        }
        Image { width, height, rgba }
    }

    /// A copy no wider or taller than `side` pixels, each pixel the average
    /// of those it covers; itself when it already fits.
    pub fn shrunk_to(&self, side: u32) -> Image {
        let longest = self.width.max(self.height);
        if longest <= side || side == 0 {
            return self.clone();
        }
        let scale = f64::from(longest) / f64::from(side);
        let width = ((f64::from(self.width) / scale).round() as u32).max(1);
        let height = ((f64::from(self.height) / scale).round() as u32).max(1);
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        let span = |i: u32, of: u32, full: u32| {
            let from = (u64::from(i) * u64::from(full) / u64::from(of)) as u32;
            let to = ((u64::from(i) + 1) * u64::from(full) / u64::from(of)) as u32;
            from..to.max(from + 1).min(full)
        };
        for y in 0..height {
            let rows = span(y, height, self.height);
            for x in 0..width {
                let cols = span(x, width, self.width);
                let mut sum = [0u64; 4];
                let mut count = 0u64;
                for sy in rows.clone() {
                    for sx in cols.clone() {
                        let i = (sy as usize * self.width as usize + sx as usize) * 4;
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += u64::from(self.rgba[i + c]);
                        }
                        count += 1;
                    }
                }
                rgba.extend(sum.iter().map(|s| (s / count.max(1)) as u8));
            }
        }
        Image { width, height, rgba }
    }

    /// The picture as a PNG file.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        // Screens compress well; this is quick and still small.
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().context("could not start the PNG")?;
        writer.write_image_data(&self.rgba).context("could not write the PNG")?;
        writer.finish().context("could not finish the PNG")?;
        Ok(out)
    }

    /// Reads a PNG file of any colour type and depth.
    pub fn from_png(bytes: &[u8]) -> Result<Image> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
        let mut reader = decoder.read_info().context("not a PNG file")?;
        let size = reader.output_buffer_size().context("the PNG is too large")?;
        let mut buf = vec![0; size];
        let info = reader.next_frame(&mut buf).context("could not read the PNG")?;
        buf.truncate(info.buffer_size());
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => buf
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|[r, g, b]| [*r, *g, *b, 255])
                .collect(),
            png::ColorType::GrayscaleAlpha => buf
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|[g, a]| [*g, *g, *g, *a])
                .collect(),
            png::ColorType::Grayscale => buf.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
            png::ColorType::Indexed => bail!("the PNG's palette was not expanded"),
        };
        Ok(Image {
            width: info.width,
            height: info.height,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture whose every pixel tells where it is: red = x, green = y.
    fn gradient(width: u32, height: u32) -> Image {
        let rgba = (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| [x as u8, y as u8, 7, 255]))
            .collect();
        Image { width, height, rgba }
    }

    #[test]
    fn packed_pixels_become_rgba() {
        // Two BGRX pixels and two bytes of row padding, then a second row.
        let data = [3, 2, 1, 0, 30, 20, 10, 0, 9, 9, 6, 5, 4, 0, 60, 50, 40, 0, 9, 9];
        let image = Image::from_packed(2, 2, 10, &data, Packing::BGRX).unwrap();
        assert_eq!(
            image.rgba,
            [1, 2, 3, 255, 10, 20, 30, 255, 4, 5, 6, 255, 40, 50, 60, 255]
        );
        // Big-endian RGB in the low bytes; 5-6-5 style masks are scaled up.
        let be = Packing {
            red: 0xff_0000,
            green: 0xff00,
            blue: 0xff,
            big_endian: true,
        };
        assert_eq!(
            Image::from_packed(1, 1, 4, &[0, 1, 2, 3], be).unwrap().rgba,
            [1, 2, 3, 255]
        );
        let small = Packing {
            red: 0xf800,
            green: 0x07e0,
            blue: 0x001f,
            big_endian: false,
        };
        let white = 0xffffu32.to_le_bytes();
        assert_eq!(
            Image::from_packed(1, 1, 4, &white, small).unwrap().rgba,
            [255, 255, 255, 255]
        );
        assert!(Image::from_packed(2, 2, 8, &data[..12], Packing::BGRX).is_err());
    }

    #[test]
    fn crops_and_shrinks() {
        let image = gradient(10, 6);
        let part = image.crop(PxRect {
            x: 2,
            y: 3,
            width: 4,
            height: 2,
        });
        assert_eq!((part.width, part.height), (4, 2));
        assert_eq!(&part.rgba[..4], &[2, 3, 7, 255]);
        assert_eq!(&part.rgba[part.rgba.len() - 4..], &[5, 4, 7, 255]);
        // A rectangle reaching past the edge is cut to the picture.
        let edge = image.crop(PxRect {
            x: 8,
            y: 5,
            width: 10,
            height: 10,
        });
        assert_eq!((edge.width, edge.height), (2, 1));

        let small = image.shrunk_to(5);
        assert_eq!((small.width, small.height), (5, 3));
        // The first pixel averages x 0..2 and y 0..2.
        assert_eq!(&small.rgba[..4], &[0, 0, 7, 255]);
        assert_eq!(image.shrunk_to(20), image);
    }

    #[test]
    fn png_round_trip() {
        let image = gradient(7, 3);
        let png = image.to_png().unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(Image::from_png(&png).unwrap(), image);
        assert!(Image::from_png(b"not a png").is_err());
    }
}
