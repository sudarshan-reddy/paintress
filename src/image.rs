use std::path::Path;
use std::str::FromStr;

use image::{DynamicImage, GenericImageView, RgbImage};

use crate::error::{PaintressError, Result};
use crate::palette::{self, Color};

/// Display hardware dimensions.
pub const DISPLAY_WIDTH: u32 = 800;
pub const DISPLAY_HEIGHT: u32 = 480;

/// Rotation applied to a tile before sending to a display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    #[default]
    None,
    Cw90,
    Ccw90,
    Flip180,
}

impl Rotation {
    /// The canvas dimensions this rotation requires, given the display's native (w, h).
    /// A 90° rotation means the tile on the canvas is portrait (h×w).
    pub fn canvas_dims(self, native_w: u32, native_h: u32) -> (u32, u32) {
        match self {
            Rotation::None | Rotation::Flip180 => (native_w, native_h),
            Rotation::Cw90 | Rotation::Ccw90 => (native_h, native_w),
        }
    }

    /// Apply this rotation to indexed pixel data.
    /// Input is row-major `src_w × src_h`, output is row-major in rotated dimensions.
    pub fn apply(self, data: &[u8], src_w: u32, src_h: u32) -> Vec<u8> {
        let w = src_w as usize;
        let h = src_h as usize;
        match self {
            Rotation::None => data.to_vec(),
            Rotation::Cw90 => {
                // (x, y) → (h-1-y, x), output is h×w
                let mut out = vec![0u8; w * h];
                for y in 0..h {
                    for x in 0..w {
                        out[x * h + (h - 1 - y)] = data[y * w + x];
                    }
                }
                out
            }
            Rotation::Ccw90 => {
                // (x, y) → (y, w-1-x), output is h×w
                let mut out = vec![0u8; w * h];
                for y in 0..h {
                    for x in 0..w {
                        out[(w - 1 - x) * h + y] = data[y * w + x];
                    }
                }
                out
            }
            Rotation::Flip180 => {
                let mut out = data.to_vec();
                out.reverse();
                out
            }
        }
    }
}

impl FromStr for Rotation {
    type Err = PaintressError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "none" | "" => Ok(Rotation::None),
            "cw90" | "cw" => Ok(Rotation::Cw90),
            "ccw90" | "ccw" => Ok(Rotation::Ccw90),
            "flip180" | "flip" => Ok(Rotation::Flip180),
            other => Err(PaintressError::InvalidRotation(other.to_owned())),
        }
    }
}

impl std::fmt::Display for Rotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rotation::None => write!(f, "none"),
            Rotation::Cw90 => write!(f, "cw90"),
            Rotation::Ccw90 => write!(f, "ccw90"),
            Rotation::Flip180 => write!(f, "flip180"),
        }
    }
}

/// A dithered image in indexed palette form.
pub struct IndexedImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // row-major palette codes
}

impl IndexedImage {
    /// Load an image, resize, optionally boost saturation, and dither to palette.
    pub fn from_file(path: &Path, width: u32, height: u32, saturation: f32) -> Result<Self> {
        let img = image::open(path)?;
        let img = img.resize_exact(width, height, image::imageops::FilterType::Lanczos3);

        let mut pixels = Self::to_f64_pixels(&img, saturation);

        eprintln!(
            "  dithering {path} to {width}x{height}...",
            path = path.display()
        );
        let data = palette::dither(&mut pixels, width, height);

        Ok(IndexedImage {
            width,
            height,
            data,
        })
    }

    /// Extract a rectangular tile from this image.
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> IndexedImage {
        let mut data = vec![0u8; (w * h) as usize];
        for row in 0..h {
            let src_start = ((y + row) * self.width + x) as usize;
            let dst_start = (row * w) as usize;
            data[dst_start..dst_start + w as usize]
                .copy_from_slice(&self.data[src_start..src_start + w as usize]);
        }
        IndexedImage {
            width: w,
            height: h,
            data,
        }
    }

    /// Rotate this tile and pack to 4bpp for the display.
    pub fn pack_rotated(&self, rotation: Rotation) -> Vec<u8> {
        let rotated = rotation.apply(&self.data, self.width, self.height);
        palette::pack_4bpp(&rotated)
    }

    /// Convert to an RGB image for preview.
    pub fn to_rgb(&self) -> RgbImage {
        let mut img = RgbImage::new(self.width, self.height);
        for (i, &code) in self.data.iter().enumerate() {
            let x = (i as u32) % self.width;
            let y = (i as u32) / self.width;
            let color = match code {
                0 => Color::Black,
                1 => Color::White,
                2 => Color::Yellow,
                3 => Color::Red,
                4 => Color::Orange,
                5 => Color::Blue,
                6 => Color::Green,
                _ => Color::White,
            };
            let [r, g, b] = color.rgb_u8();
            img.put_pixel(x, y, image::Rgb([r, g, b]));
        }
        img
    }

    fn to_f64_pixels(img: &DynamicImage, saturation: f32) -> Vec<[f64; 3]> {
        let (w, h) = img.dimensions();
        let rgb = img.to_rgb8();
        let mut pixels = Vec::with_capacity((w * h) as usize);

        for y in 0..h {
            for x in 0..w {
                let p = rgb.get_pixel(x, y);
                let [r, g, b] = [p[0] as f64, p[1] as f64, p[2] as f64];

                if (saturation - 1.0).abs() < f32::EPSILON {
                    pixels.push([r, g, b]);
                } else {
                    // Simple saturation boost: blend toward/away from luminance
                    let lum = 0.299 * r + 0.587 * g + 0.114 * b;
                    let s = saturation as f64;
                    pixels.push([
                        lum + (r - lum) * s,
                        lum + (g - lum) * s,
                        lum + (b - lum) * s,
                    ]);
                }
            }
        }

        pixels
    }
}
