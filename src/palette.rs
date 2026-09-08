/// Spectra 6 six-color palette and Floyd-Steinberg dithering.

/// Hardware color codes for E Ink Spectra 6 panels.
///
/// Spectra 6 is a *six* color process and the controller has no entry for
/// code `0x04`. Seeed_GFX's `COLOR_GET` maps to exactly this set for both the
/// ED2208 (7.3") and T133A01 (13.3") panels, so an orange at code 4 was never
/// a color either panel could render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    White = 1,
    Yellow = 2,
    Red = 3,
    Blue = 5,
    Green = 6,
}

impl Color {
    pub const ALL: [Color; 6] = [
        Color::Black,
        Color::White,
        Color::Yellow,
        Color::Red,
        Color::Blue,
        Color::Green,
    ];

    pub fn rgb(self) -> [f64; 3] {
        match self {
            Color::Black => [0.0, 0.0, 0.0],
            Color::White => [255.0, 255.0, 255.0],
            Color::Yellow => [255.0, 230.0, 0.0],
            Color::Red => [200.0, 0.0, 0.0],
            Color::Blue => [0.0, 0.0, 255.0],
            Color::Green => [0.0, 128.0, 0.0],
        }
    }

    pub fn rgb_u8(self) -> [u8; 3] {
        match self {
            Color::Black => [0, 0, 0],
            Color::White => [255, 255, 255],
            Color::Yellow => [255, 230, 0],
            Color::Red => [200, 0, 0],
            Color::Blue => [0, 0, 255],
            Color::Green => [0, 128, 0],
        }
    }
}

/// Find the nearest palette color to an RGB pixel (Euclidean distance).
fn nearest_color(r: f64, g: f64, b: f64) -> Color {
    let mut best = Color::Black;
    let mut best_dist = f64::MAX;
    for &c in &Color::ALL {
        let [cr, cg, cb] = c.rgb();
        let dist = (r - cr).powi(2) + (g - cg).powi(2) + (b - cb).powi(2);
        if dist < best_dist {
            best_dist = dist;
            best = c;
        }
    }
    best
}

/// Floyd-Steinberg dither an RGB image buffer to the 6-color Spectra 6 palette.
///
/// `pixels` is row-major RGB as f64 (will be mutated for error diffusion).
/// Returns a flat vec of palette codes, row-major.
pub fn dither(pixels: &mut Vec<[f64; 3]>, width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h];

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let [r, g, b] = pixels[idx];
            let color = nearest_color(r, g, b);
            out[idx] = color as u8;

            let [cr, cg, cb] = color.rgb();
            let err = [r - cr, g - cg, b - cb];

            // Distribute error to neighbors
            if x + 1 < w {
                let i = idx + 1;
                pixels[i][0] += err[0] * 7.0 / 16.0;
                pixels[i][1] += err[1] * 7.0 / 16.0;
                pixels[i][2] += err[2] * 7.0 / 16.0;
            }
            if y + 1 < h {
                if x > 0 {
                    let i = (y + 1) * w + (x - 1);
                    pixels[i][0] += err[0] * 3.0 / 16.0;
                    pixels[i][1] += err[1] * 3.0 / 16.0;
                    pixels[i][2] += err[2] * 3.0 / 16.0;
                }
                {
                    let i = (y + 1) * w + x;
                    pixels[i][0] += err[0] * 5.0 / 16.0;
                    pixels[i][1] += err[1] * 5.0 / 16.0;
                    pixels[i][2] += err[2] * 5.0 / 16.0;
                }
                if x + 1 < w {
                    let i = (y + 1) * w + (x + 1);
                    pixels[i][0] += err[0] * 1.0 / 16.0;
                    pixels[i][1] += err[1] * 1.0 / 16.0;
                    pixels[i][2] += err[2] * 1.0 / 16.0;
                }
            }
        }

        if y % 48 == 0 {
            eprintln!("  dithering... {}%", y * 100 / h);
        }
    }

    out
}

/// Pack indexed pixels to 4bpp: high nibble = left pixel, low nibble = right pixel.
pub fn pack_4bpp(indexed: &[u8]) -> Vec<u8> {
    indexed
        .chunks(2)
        .map(|pair| {
            let hi = pair[0] & 0x0F;
            let lo = if pair.len() > 1 { pair[1] & 0x0F } else { 0 };
            (hi << 4) | lo
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spectra 6 has no code 0x04. Emitting one puts an undefined value in the
    /// 4bpp stream, which the panel is free to render as anything.
    #[test]
    fn palette_codes_are_valid_spectra6() {
        let codes: Vec<u8> = Color::ALL.iter().map(|&c| c as u8).collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 5, 6]);
        assert!(!codes.contains(&4), "0x04 is not a Spectra 6 color");
    }

    #[test]
    fn dither_only_emits_valid_codes() {
        // A gradient exercises every branch of nearest_color + error diffusion.
        let (w, h) = (64u32, 64u32);
        let mut pixels: Vec<[f64; 3]> = (0..h)
            .flat_map(|y| {
                (0..w).map(move |x| [(x * 4) as f64, (y * 4) as f64, ((x + y) * 2) as f64])
            })
            .collect();

        let out = dither(&mut pixels, w, h);
        assert_eq!(out.len(), (w * h) as usize);
        for &code in &out {
            assert!(
                matches!(code, 0 | 1 | 2 | 3 | 5 | 6),
                "dither emitted invalid palette code {code}"
            );
        }
    }

    #[test]
    fn pack_4bpp_packs_two_pixels_per_byte() {
        assert_eq!(pack_4bpp(&[0x0, 0x1, 0x6, 0x5]), vec![0x01, 0x65]);
    }
}
