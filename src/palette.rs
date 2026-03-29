/// ED2208 Spectra 6 seven-color palette and Floyd-Steinberg dithering.

/// Hardware color codes for the ED2208 display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    White = 1,
    Yellow = 2,
    Red = 3,
    Orange = 4,
    Blue = 5,
    Green = 6,
}

impl Color {
    pub const ALL: [Color; 7] = [
        Color::Black,
        Color::White,
        Color::Yellow,
        Color::Red,
        Color::Orange,
        Color::Blue,
        Color::Green,
    ];

    pub fn rgb(self) -> [f64; 3] {
        match self {
            Color::Black => [0.0, 0.0, 0.0],
            Color::White => [255.0, 255.0, 255.0],
            Color::Yellow => [255.0, 230.0, 0.0],
            Color::Red => [200.0, 0.0, 0.0],
            Color::Orange => [255.0, 140.0, 0.0],
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
            Color::Orange => [255, 140, 0],
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

/// Floyd-Steinberg dither an RGB image buffer to the 7-color ED2208 palette.
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
