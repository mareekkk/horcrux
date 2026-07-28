//! The EPUB cover, rendered as ink-on-parchment art.
//!
//! No image asset is shipped: the cover is drawn at build time from the same
//! Dancing Script font the diary writes in, plus a hand-drawn quill. It reads
//! as warm ink on aged paper and stays legible on grayscale e-ink.

use ab_glyph::{Font, FontArc, Glyph, ScaleFont};

const FONT_BYTES: &[u8] = include_bytes!("../fonts/DancingScript.ttf");

const W: u32 = 1200;
const H: u32 = 1600;

/// Parchment RGB.
const PAPER: [u8; 3] = [232, 222, 198];
/// Near-black warm ink.
const INK: [u8; 3] = [42, 34, 22];
/// Faded sepia for secondary text.
const SEPIA: [u8; 3] = [104, 86, 54];

struct Canvas {
    buf: Vec<u8>,
}

impl Canvas {
    fn new() -> Canvas {
        Canvas {
            buf: vec![0; (W * H * 4) as usize],
        }
    }

    #[inline]
    fn idx(x: i32, y: i32) -> Option<usize> {
        if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
            Some(((y as u32 * W + x as u32) * 4) as usize)
        } else {
            None
        }
    }

    /// Solid overwrite.
    fn set(&mut self, x: i32, y: i32, [r, g, b]: [u8; 3]) {
        if let Some(i) = Self::idx(x, y) {
            self.buf[i] = r;
            self.buf[i + 1] = g;
            self.buf[i + 2] = b;
            self.buf[i + 3] = 255;
        }
    }

    /// Alpha-composite a coverage value over the existing pixel.
    fn blend(&mut self, x: i32, y: i32, [r, g, b]: [u8; 3], a: f32) {
        if let Some(i) = Self::idx(x, y) {
            let ia = 1.0 - a;
            self.buf[i] = (r as f32 * a + self.buf[i] as f32 * ia) as u8;
            self.buf[i + 1] = (g as f32 * a + self.buf[i + 1] as f32 * ia) as u8;
            self.buf[i + 2] = (b as f32 * a + self.buf[i + 2] as f32 * ia) as u8;
            self.buf[i + 3] = 255;
        }
    }

    fn fill(&mut self, [r, g, b]: [u8; 3]) {
        for px in self.buf.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = 255;
        }
    }

    fn disc(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 3]) {
        let r2 = r * r;
        let x0 = (cx - r).floor() as i32;
        let x1 = (cx + r).ceil() as i32;
        let y0 = (cy - r).floor() as i32;
        let y1 = (cy + r).ceil() as i32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                if dx * dx + dy * dy <= r2 {
                    self.set(x, y, c);
                }
            }
        }
    }

    /// A thick stroke between two points, built from overlapping discs — gives
    /// an organic, pen-like weight. `jig` adds a little hand-drawn wobble.
    #[allow(clippy::too_many_arguments)]
    fn stroke(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, c: [u8; 3], jig: f32) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt();
        let steps = (len / (r * 0.4)).ceil() as i32;
        let mut seed: u32 = ((x0 * 13.0 + y0 * 7.0 + x1 + y1) as u32).max(1);
        for s in 0..=steps {
            let t = if steps == 0 {
                0.0
            } else {
                s as f32 / steps as f32
            };
            let mut lcg = || {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 8) as f32 / 16777216.0 - 0.5) * 2.0
            };
            let nx = -dy / len;
            let ny = dx / len;
            let off = if jig > 0.0 { lcg() * jig } else { 0.0 };
            let taper = 1.0 - (2.0 * t - 1.0).abs() * 0.18; // slight taper at ends
            self.disc(x0 + dx * t + nx * off, y0 + dy * t + ny * off, r * taper, c);
        }
    }

    /// Solid, antialiased centered text.
    fn text(&mut self, font: &FontArc, text: &str, px: f32, cx: f32, baseline: f32, c: [u8; 3]) {
        let scaled = font.as_scaled(px);
        let total: f32 = text
            .chars()
            .map(|ch| scaled.h_advance(font.glyph_id(ch)))
            .sum();
        let mut x = cx - total / 2.0;
        for ch in text.chars() {
            let gid = font.glyph_id(ch);
            let glyph: Glyph = gid.with_scale_and_position(px, ab_glyph::point(x, baseline));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bb = outlined.px_bounds();
                let min_x = bb.min.x.round() as i32;
                let min_y = bb.min.y.round() as i32;
                outlined.draw(|gx, gy, cov| {
                    if cov > 0.04 {
                        self.blend(min_x + gx as i32, min_y + gy as i32, c, cov);
                    }
                });
            }
            x += scaled.h_advance(gid);
        }
    }
}

/// Render the cover and return it as PNG bytes.
pub fn cover_png() -> Vec<u8> {
    let font = FontArc::try_from_slice(FONT_BYTES).expect("Dancing Script font");
    let mut c = Canvas::new();

    // Aged paper: base fill, soft radial vignette, faint grain.
    c.fill(PAPER);
    let cx = W as f32 / 2.0;
    let cy = H as f32 / 2.0;
    let max_d = (cx * cx + cy * cy).sqrt();
    let mut seed: u32 = 0x9E37_79B9;
    for y in 0..H {
        for x in 0..W {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d = (dx * dx + dy * dy).sqrt() / max_d; // 0 center .. 1 corner
            let shade = 1.0 - (d * d * 0.42).min(0.42); // darken edges ~ up to 42%
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let grain = ((seed >> 16) as f32 / 65536.0 - 0.5) * 12.0; // ±6
            let p = |base: u8| -> u8 {
                ((base as f32 * shade + grain).round().clamp(0.0, 255.0)) as u8
            };
            let i = ((y * W + x) * 4) as usize;
            c.buf[i] = p(PAPER[0]);
            c.buf[i + 1] = p(PAPER[1]);
            c.buf[i + 2] = p(PAPER[2]);
            c.buf[i + 3] = 255;
        }
    }

    // Double ink frame, slightly hand-drawn.
    let inset = 70.0;
    let w = W as f32 - inset * 2.0;
    let h = H as f32 - inset * 2.0;
    for o in [0.0, 13.0] {
        let x0 = inset + o;
        let y0 = inset + o;
        let x1 = x0 + w - o * 2.0;
        let y1 = y0 + h - o * 2.0;
        let r = 2.4;
        let g = 0.7;
        c.stroke(x0, y0, x1, y0, r, INK, g);
        c.stroke(x1, y0, x1, y1, r, INK, g);
        c.stroke(x1, y1, x0, y1, r, INK, g);
        c.stroke(x0, y1, x0, y0, r, INK, g);
    }

    // Title.
    c.text(&font, "Horcrux", 230.0, cx, 560.0, INK);

    // Flourish divider under the title.
    c.stroke(360.0, 660.0, 560.0, 660.0, 1.8, INK, 0.5);
    c.stroke(640.0, 660.0, 840.0, 660.0, 1.8, INK, 0.5);
    for k in -2..=2 {
        // a small diamond ornament
        c.stroke(
            600.0 + k as f32 * 2.0,
            652.0,
            604.0 + k as f32 * 2.0,
            660.0,
            1.4,
            INK,
            0.0,
        );
        c.stroke(
            604.0 + k as f32 * 2.0,
            660.0,
            600.0 + k as f32 * 2.0,
            668.0,
            1.4,
            INK,
            0.0,
        );
    }

    // Subtitle.
    c.text(&font, "an enchanted diary", 70.0, cx, 760.0, SEPIA);

    // A quill, nib down toward an inkwell.
    draw_quill(&mut c, cx, 1000.0);

    // Tagline.
    c.text(
        &font,
        "Write, and the page writes back.",
        48.0,
        cx,
        1380.0,
        SEPIA,
    );

    encode_png(&c.buf)
}

fn draw_quill(c: &mut Canvas, cx: f32, top: f32) {
    // Rachis (shaft), curving gently down to the nib.
    let shaft = [
        (cx + 26.0, top),
        (cx + 14.0, top + 70.0),
        (cx + 2.0, top + 150.0),
        (cx - 6.0, top + 230.0),
    ];
    for w in shaft.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        c.stroke(x0, y0, x1, y1, 3.2, INK, 0.5);
    }
    // Barbs fanning off the upper shaft (left side, longer).
    let barbs = [
        ((cx + 24.0, top + 10.0), (cx - 70.0, top - 24.0)),
        ((cx + 20.0, top + 38.0), (cx - 96.0, top + 6.0)),
        ((cx + 15.0, top + 66.0), (cx - 112.0, top + 40.0)),
        ((cx + 10.0, top + 94.0), (cx - 120.0, top + 76.0)),
        ((cx + 5.0, top + 122.0), (cx - 116.0, top + 114.0)),
        ((cx - 1.0, top + 150.0), (cx - 102.0, top + 152.0)),
        ((cx - 6.0, top + 178.0), (cx - 80.0, top + 188.0)),
    ];
    for ((x0, y0), (x1, y1)) in barbs {
        c.stroke(x0, y0, x1, y1, 2.0, INK, 0.6);
    }
    // Shorter right-side barbs.
    let rbarbs = [
        ((cx + 28.0, top + 18.0), (cx + 74.0, top + 0.0)),
        ((cx + 23.0, top + 48.0), (cx + 86.0, top + 36.0)),
        ((cx + 17.0, top + 78.0), (cx + 90.0, top + 76.0)),
        ((cx + 10.0, top + 108.0), (cx + 86.0, top + 116.0)),
    ];
    for ((x0, y0), (x1, y1)) in rbarbs {
        c.stroke(x0, y0, x1, y1, 1.7, INK, 0.5);
    }
    // Nib.
    c.stroke(cx - 8.0, top + 228.0, cx - 8.0, top + 258.0, 3.4, INK, 0.0);
    c.stroke(cx - 8.0, top + 258.0, cx - 14.0, top + 268.0, 2.4, INK, 0.0);
    c.stroke(cx - 8.0, top + 258.0, cx - 2.0, top + 268.0, 2.4, INK, 0.0);
    // Inkwell.
    let bx = cx - 8.0;
    let by = top + 286.0;
    c.disc(bx, by, 26.0, INK);
    c.disc(bx, by - 10.0, 20.0, INK);
    c.disc(bx, by - 10.0, 12.0, PAPER); // hollow rim
}

fn encode_png(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, W, H);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("png header");
        writer.write_image_data(rgba).expect("png encode");
    }
    out
}
