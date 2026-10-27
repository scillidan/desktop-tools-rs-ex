//! Blue frame glyph for the exe and tray. The canvas and ICO encoder are
//! copied from desktop-tools-rs src/tray.rs; the body is hatched instead of
//! filled to mean an OCR frame rather than a window.

const BLUE: [u8; 4] = [37, 99, 235, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];
const SS: i32 = 4;
const AUTHORED: i32 = 32;

fn draw(n: i32) -> Vec<u8> {
    let mut c = Canvas::new(n);
    c.rect(5.0, 5.0, 28.0, 12.0, BLUE);
    c.rect(8.0, 8.0, 25.0, 12.0, CLEAR);
    // The whole body is hatched: a frame, not a filled window.
    for y in 12..27 {
        for x in 5..28 {
            if (x + y) % 4 < 2 {
                c.rect(x as f32, y as f32, (x + 1) as f32, (y + 1) as f32, BLUE);
            }
        }
    }
    c.downsample()
}

struct Canvas {
    n: i32,
    px: Vec<[u8; 4]>,
}

impl Canvas {
    fn new(n: i32) -> Self {
        Canvas {
            n,
            px: vec![[0; 4]; (n * SS * n * SS) as usize],
        }
    }

    fn set(&mut self, x: i32, y: i32, color: [u8; 4]) {
        let w = self.n * SS;
        if (0..w).contains(&x) && (0..w).contains(&y) {
            self.px[(y * w + x) as usize] = color;
        }
    }

    fn scale(&self) -> f32 {
        SS as f32 * self.n as f32 / AUTHORED as f32
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: [u8; 4]) {
        let s = self.scale();
        for y in (y0 * s).round() as i32..(y1 * s).round() as i32 {
            for x in (x0 * s).round() as i32..(x1 * s).round() as i32 {
                self.set(x, y, color);
            }
        }
    }

    fn downsample(self) -> Vec<u8> {
        let n = self.n;
        let samples = (SS * SS) as u32;
        let mut out = vec![0u8; (n * n * 4) as usize];
        for y in 0..n {
            for x in 0..n {
                let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
                for dy in 0..SS {
                    for dx in 0..SS {
                        let p = self.px[((y * SS + dy) * n * SS + x * SS + dx) as usize];
                        let al = p[3] as u32;
                        r += p[0] as u32 * al;
                        g += p[1] as u32 * al;
                        b += p[2] as u32 * al;
                        a += al;
                    }
                }
                let i = ((y * n + x) * 4) as usize;
                let a = a.max(1);
                out[i] = (r / a) as u8;
                out[i + 1] = (g / a) as u8;
                out[i + 2] = (b / a) as u8;
                out[i + 3] = (a / samples) as u8;
            }
        }
        out
    }
}

/// 48/32/16 px BMP-DIB entries with straight-alpha BGRA and empty AND masks.
#[allow(dead_code)] // used by the explicit asset-regeneration test
pub fn ico_bytes() -> Vec<u8> {
    const SIZES: [u32; 3] = [48, 32, 16];
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&n| draw(n as i32)).collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0, SIZES.len() as u8, 0]);
    let mut offset = (6 + 16 * SIZES.len()) as u32;
    for &n in &SIZES {
        let mask_row = n.div_ceil(32) * 4;
        let size = 40 + n * n * 4 + mask_row * n;
        ico.extend_from_slice(&[n as u8, n as u8, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&size.to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += size;
    }
    for (i, &n) in SIZES.iter().enumerate() {
        let px = &images[i];
        let mask_row = (n.div_ceil(32) * 4) as usize;
        ico.extend_from_slice(&40u32.to_le_bytes());
        ico.extend_from_slice(&n.to_le_bytes());
        ico.extend_from_slice(&(n * 2).to_le_bytes());
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&[0; 24]);
        for y in (0..n).rev() {
            for x in 0..n {
                let at = ((y * n + x) * 4) as usize;
                ico.extend_from_slice(&[px[at + 2], px[at + 1], px[at], px[at + 3]]);
            }
        }
        ico.resize(ico.len() + mask_row * n as usize, 0);
    }
    ico
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: i32 = 32;

    #[test]
    fn frame_is_hatched_not_filled() {
        let px = draw(N);
        let alpha = |x: i32, y: i32| px[((y * N + x) * 4 + 3) as usize];
        assert_eq!(alpha(5, 5), 255, "title bar is opaque");
        assert_eq!(alpha(16, 9), 0, "title bar stays hollow");
        assert_eq!(alpha(16, 28), 0, "glyph does not crowd the canvas");
        let body: Vec<u8> = (12..27)
            .flat_map(|y| (5..28).map(move |x| alpha(x, y)))
            .collect();
        assert!(body.contains(&255), "hatch has blue strokes");
        assert!(body.contains(&0), "hatch leaves transparent gaps");
    }

    #[test]
    fn ico_has_three_coherent_entries() {
        let ico = ico_bytes();
        assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
        assert_eq!(ico[4], 3);
        let mut expected_offset = (6 + 16 * 3) as u32;
        for (i, size) in [48u8, 32, 16].iter().enumerate() {
            let at = 6 + i * 16;
            assert_eq!(ico[at], *size);
            let size = u32::from_le_bytes(ico[at + 8..at + 12].try_into().unwrap());
            let offset = u32::from_le_bytes(ico[at + 12..at + 16].try_into().unwrap());
            assert_eq!(offset, expected_offset);
            expected_offset += size;
        }
        assert_eq!(expected_offset, ico.len() as u32);
    }

    #[test]
    #[ignore = "regenerates assets/dt-ocr.ico after editing the glyph"]
    fn write_icon_asset() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dt-ocr.ico"), ico_bytes()).unwrap();
    }
}
