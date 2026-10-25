//! Read-back images, the pixel comparison, and PNG output.

use std::path::Path;

/// Per-channel difference (of 255, in the sRGB-encoded bytes the screen
/// shows) up to which two pixels count as equal. Two correct translations of
/// the same shader differ by a few ULPs per operation: invisible on smooth
/// gradients, where the rounded bytes differ by 1–2, but a hash like
/// `fract(sin(x) * 43758.5)` or a raymarch that just misses an edge turns
/// them into arbitrary values. 8/255 (≈3%) clears the first kind with margin
/// while anything a viewer would call a different colour stays above it.
pub const PIXEL_TOLERANCE: u8 = 8;

/// Share of pixels allowed above [`PIXEL_TOLERANCE`] before two renders count
/// as different: room for the scattered pixels of the second kind above
/// (hash noise, edges, chaotic iterations), while a mistranslated
/// expression, which is wrong across whole regions, is far beyond it.
pub const MAX_DIFFERING_SHARE: f64 = 0.01;

/// An 8-bit RGBA image, rows top to bottom.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn write_png(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&self.rgba)
            .map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())
    }
}

/// How two renders of one shader differ (alpha is ignored: the app
/// composites opaque).
#[derive(Clone, Debug)]
pub struct Diff {
    /// Largest per-channel difference over all pixels.
    pub max: u8,
    /// Pixels whose largest channel difference exceeds [`PIXEL_TOLERANCE`].
    pub differing: usize,
    pub pixels: usize,
    /// |a − b| per channel, amplified 16× so ULP-level noise shows up as
    /// dark grey and real differences saturate.
    pub image: Image,
}

impl Diff {
    pub fn new(a: &Image, b: &Image) -> Diff {
        assert_eq!((a.width, a.height), (b.width, b.height), "image sizes");
        let mut max = 0;
        let mut differing = 0;
        let mut rgba = Vec::with_capacity(a.rgba.len());
        for (pa, pb) in a
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.rgba.as_chunks::<4>().0)
        {
            let d: [u8; 3] = std::array::from_fn(|c| pa[c].abs_diff(pb[c]));
            let m = d.into_iter().max().unwrap_or(0);
            max = max.max(m);
            differing += usize::from(m > PIXEL_TOLERANCE);
            rgba.extend(d.map(|v| v.saturating_mul(16)));
            rgba.push(255);
        }
        Diff {
            max,
            differing,
            pixels: a.rgba.len() / 4,
            image: Image {
                width: a.width,
                height: a.height,
                rgba,
            },
        }
    }

    /// Share of pixels above [`PIXEL_TOLERANCE`].
    pub fn share(&self) -> f64 {
        self.differing as f64 / self.pixels.max(1) as f64
    }

    /// Whether the renders count as the same picture.
    pub fn matches(&self) -> bool {
        self.share() <= MAX_DIFFERING_SHARE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Image {
        let pixel = [rgb[0], rgb[1], rgb[2], 255];
        Image {
            width,
            height,
            rgba: pixel.repeat((width * height) as usize),
        }
    }

    #[test]
    fn diff_counts_pixels_over_the_tolerance() {
        let a = solid(10, 10, [100, 100, 100]);
        let mut b = solid(10, 10, [100, 100, 100 + PIXEL_TOLERANCE]);
        let near = Diff::new(&a, &b);
        assert_eq!((near.max, near.differing), (PIXEL_TOLERANCE, 0));
        assert!(near.matches());
        // One pixel of 100 is exactly at the allowed share, two are over it.
        b.rgba[0] = 0;
        assert!(Diff::new(&a, &b).matches());
        b.rgba[4] = 0;
        let far = Diff::new(&a, &b);
        assert_eq!((far.max, far.differing), (100, 2));
        assert!(!far.matches());
        assert_eq!(far.image.rgba[..4], [255, 0, 128, 255]);
    }
}
