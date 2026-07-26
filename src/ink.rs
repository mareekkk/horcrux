//! Stored ink strokes, the pen brush, the eraser, page rasterization for
//! the oracle, and the hash-staged dissolve animation.

use crate::fb;
use crate::surface::{Rect, Surface};
use std::io;

/// One stamped ink point: x, y, brush radius.
pub type Point = (i32, i32, i32);

pub const ERASER_RADIUS: i32 = 22;

#[derive(Default)]
pub struct Ink {
    pub strokes: Vec<Vec<Point>>,
}

impl Ink {
    pub fn new() -> Ink {
        Ink::default()
    }

    pub fn is_empty(&self) -> bool {
        self.strokes.iter().all(|s| s.is_empty())
    }

    pub fn clear(&mut self) {
        self.strokes.clear();
    }

    pub fn start_stroke(&mut self) {
        self.strokes.push(Vec::new());
    }

    pub fn add_point(&mut self, p: Point) {
        if self.strokes.is_empty() {
            self.start_stroke();
        }
        self.strokes.last_mut().unwrap().push(p);
    }

    /// Bounding box of all ink, expanded by brush radius.
    pub fn bbox(&self) -> Option<Rect> {
        let mut bb: Option<Rect> = None;
        for stroke in &self.strokes {
            for &(x, y, r) in stroke {
                let pr = Rect::around(x, y, r);
                match &mut bb {
                    Some(b) => b.include(pr),
                    None => bb = Some(pr),
                }
            }
        }
        bb.map(|b| b.clamped())
    }

    /// Remove every stored point within `radius` of the segment (x0,y0)-(x1,y1),
    /// splitting strokes where the middle was erased.
    pub fn erase_segment(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, radius: i32) {
        let r2 = (radius * radius) as f32;
        let mut rebuilt: Vec<Vec<Point>> = Vec::new();
        for stroke in self.strokes.drain(..) {
            let mut run: Vec<Point> = Vec::new();
            for p in stroke {
                if dist2_to_segment(p.0, p.1, x0, y0, x1, y1) <= r2 {
                    if !run.is_empty() {
                        rebuilt.push(std::mem::take(&mut run));
                    }
                } else {
                    run.push(p);
                }
            }
            if !run.is_empty() {
                rebuilt.push(run);
            }
        }
        self.strokes = rebuilt;
    }
}

fn dist2_to_segment(px: i32, py: i32, x0: i32, y0: i32, x1: i32, y1: i32) -> f32 {
    let (px, py, x0, y0, x1, y1) = (
        px as f32, py as f32, x0 as f32, y0 as f32, x1 as f32, y1 as f32,
    );
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-6 {
        0.0
    } else {
        (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (x0 + t * dx, y0 + t * dy);
    (px - cx) * (px - cx) + (py - cy) * (py - cy)
}

/// DDA brush: stamp filled circles along the segment, lerping the radius.
/// `gray` is the ink level: 0 black, 255 white (170 for conjured ink).
#[allow(clippy::too_many_arguments)]
pub fn brush_line(
    surf: &mut Surface,
    x0: i32,
    y0: i32,
    r0: i32,
    x1: i32,
    y1: i32,
    r1: i32,
    gray: u8,
) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = x0 as f32 + (x1 - x0) as f32 * t;
        let y = y0 as f32 + (y1 - y0) as f32 * t;
        let r = r0 as f32 + (r1 - r0) as f32 * t;
        surf.fill_circle(x.round() as i32, y.round() as i32, r.round() as i32, gray);
    }
    let rmax = r0.max(r1) + 1;
    let mut d = Rect::around(x0, y0, rmax);
    d.include(Rect::around(x1, y1, rmax));
    surf.mark_dirty(d);
}

/// Dithered DDA brush: like brush_line, but only the `parity` half of each
/// stamp's pixels — the materialize effect's speckle/solidify passes.
#[allow(clippy::too_many_arguments)]
pub fn brush_line_dither(
    surf: &mut Surface,
    x0: i32,
    y0: i32,
    r0: i32,
    x1: i32,
    y1: i32,
    r1: i32,
    gray: u8,
    parity: i32,
) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = x0 as f32 + (x1 - x0) as f32 * t;
        let y = y0 as f32 + (y1 - y0) as f32 * t;
        let r = r0 as f32 + (r1 - r0) as f32 * t;
        surf.fill_circle_dither(
            x.round() as i32,
            y.round() as i32,
            r.round() as i32,
            gray,
            parity,
        );
    }
    let rmax = r0.max(r1) + 1;
    let mut d = Rect::around(x0, y0, rmax);
    d.include(Rect::around(x1, y1, rmax));
    surf.mark_dirty(d);
}

// --- dissolve ----------------------------------------------------------------

/// Deterministic per-pixel stage hash: nearby pixels almost never share a
/// stage, so the ink appears to be "drunk" evenly all over the page.
fn hash_stage(x: u32, y: u32) -> u32 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 16;
    h
}

pub struct Dissolve {
    /// (pixel index, stage) for every non-white pixel; each stage filters
    /// the whole list for `stage <= current`.
    pixels: Vec<(u32, u8)>,
    pub bbox: Rect,
    /// Soft frontier: once the frontier reaches a pixel, it whitens
    /// probabilistically over this many extra stages (monotonic per pixel),
    /// so a wide band fades gradually instead of a hard wipe. 0 = hard.
    soft: u8,
}

impl Dissolve {
    /// Plan a dissolve of everything currently darker than luma 250.
    pub fn plan(shadow: &[u8], stages: u8) -> Option<Dissolve> {
        let mut pixels: Vec<(u32, u8)> = Vec::new();
        let mut bb: Option<Rect> = None;
        for (idx, &luma) in shadow.iter().enumerate() {
            if luma < 250 {
                let x = (idx as i32 % fb::WIDTH) as u32;
                let y = (idx as i32 / fb::WIDTH) as u32;
                pixels.push((idx as u32, (hash_stage(x, y) % stages as u32) as u8));
                let pr = Rect::around(x as i32, y as i32, 0);
                match &mut bb {
                    Some(b) => b.include(pr),
                    None => bb = Some(pr),
                }
            }
        }
        if pixels.is_empty() {
            return None;
        }
        Some(Dissolve {
            pixels,
            bbox: bb.unwrap().clamped(),
            soft: 0,
        })
    }

    /// Plan a dissolve in writing order. `ordered` holds the ink's stroke
    /// points in the order they were written; every ink pixel is assigned
    /// the order index of the stroke point nearest to it (via a stamped
    /// order map), so the first-written ink fades first — line by line,
    /// left to right — exactly as it appeared. The frontier is soft
    /// (`SOFT_FRONTIER` stages of probabilistic whitening).
    pub fn plan_ordered(shadow: &[u8], ordered: &[Point], stages: u8) -> Option<Dissolve> {
        if ordered.is_empty() {
            return Dissolve::plan(shadow, stages);
        }
        // Order map: for each pixel, the earliest stroke point covering it.
        let mut order_map = vec![u32::MAX; shadow.len()];
        for (i, &(x, y, r)) in ordered.iter().enumerate() {
            let rad = r + 2;
            for dy in -rad..=rad {
                for dx in -rad..=rad {
                    if dx * dx + dy * dy > rad * rad {
                        continue;
                    }
                    let (px, py) = (x + dx, y + dy);
                    if px < 0 || py < 0 || px >= fb::WIDTH || py >= fb::HEIGHT {
                        continue;
                    }
                    let pidx = (py * fb::WIDTH + px) as usize;
                    if (i as u32) < order_map[pidx] {
                        order_map[pidx] = i as u32;
                    }
                }
            }
        }
        let n = ordered.len().max(2) as u32;
        // Orders are spread over `stages - SOFT_FRONTIER` distinct stages so
        // the soft frontier of the last-written ink completes within the
        // total stage count.
        let effective = stages.saturating_sub(SOFT_FRONTIER).max(1);
        let mut pixels: Vec<(u32, u8)> = Vec::new();
        let mut bb: Option<Rect> = None;
        for (idx, &luma) in shadow.iter().enumerate() {
            if luma < 250 {
                let st = match order_map[idx] {
                    u32::MAX => effective - 1, // stray specks fade last
                    ord => ((ord as u64 * (effective as u64 - 1)) / (n as u64 - 1)) as u8,
                };
                pixels.push((idx as u32, st));
                let pr = Rect::around(idx as i32 % fb::WIDTH, idx as i32 / fb::WIDTH, 0);
                match &mut bb {
                    Some(b) => b.include(pr),
                    None => bb = Some(pr),
                }
            }
        }
        if pixels.is_empty() {
            return None;
        }
        Some(Dissolve {
            pixels,
            bbox: bb.unwrap().clamped(),
            soft: SOFT_FRONTIER,
        })
    }

    /// Whiten pixels the frontier has reached. With a soft frontier, a
    /// pixel reached `soft` stages ago whitens only if its hash falls in
    /// the growing fraction (monotonic: once white, it stays white).
    pub fn apply_stage(&self, surf: &mut Surface, stage: u8) {
        for &(idx, st) in &self.pixels {
            if st <= stage {
                if self.soft > 0 {
                    let age = stage - st;
                    if age < self.soft
                        && hash_stage(idx % fb::WIDTH as u32, idx / fb::WIDTH as u32)
                            % self.soft as u32
                            >= age as u32
                    {
                        continue; // still inside the soft band, not this stage
                    }
                }
                let x = idx as i32 % fb::WIDTH;
                let y = idx as i32 / fb::WIDTH;
                surf.set_gray(x, y, 255);
            }
        }
        surf.mark_dirty(self.bbox);
    }
}

/// Stages of probabilistic whitening at the dissolve frontier. A wide
/// frontier makes the fade read as a broad, slow, mysterious dissolve
/// rather than a mechanical letter-by-letter wipe.
const SOFT_FRONTIER: u8 = 12;

// --- page rasterization for the oracle ---------------------------------------

/// Crop the ink bbox (+20px margin), integer box-downscale so the long side
/// is ~800px, and encode an 8-bit grayscale PNG.
pub fn rasterize_page_png(shadow: &[u8], ink_bbox: Rect) -> io::Result<Vec<u8>> {
    let crop = Rect::new(
        ink_bbox.x0 - 20,
        ink_bbox.y0 - 20,
        ink_bbox.x1 + 20,
        ink_bbox.y1 + 20,
    )
    .clamped();
    let (w, h) = (crop.width(), crop.height());
    if w <= 0 || h <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty ink bbox",
        ));
    }
    let long = w.max(h);
    let f = ((long + 799) / 800).max(2); // ceil(long/800), at least 2
    let (ow, oh) = (((w + f - 1) / f) as usize, ((h + f - 1) / f) as usize);
    let f = f as usize;

    let mut out = vec![0u8; ow * oh];
    for oy in 0..oh {
        for ox in 0..ow {
            let mut sum: u32 = 0;
            let mut n: u32 = 0;
            let y0 = crop.y0 + (oy * f) as i32;
            let x0 = crop.x0 + (ox * f) as i32;
            for dy in 0..f as i32 {
                let y = y0 + dy;
                if y >= crop.y1 {
                    break;
                }
                let row = (y * fb::WIDTH) as usize;
                for dx in 0..f as i32 {
                    let x = x0 + dx;
                    if x >= crop.x1 {
                        break;
                    }
                    sum += shadow[row + x as usize] as u32;
                    n += 1;
                }
            }
            out[oy * ow + ox] = sum.checked_div(n).map(|v| v as u8).unwrap_or(255);
        }
    }

    encode_png_gray(&out, ow as u32, oh as u32)
}

fn encode_png_gray(gray: &[u8], w: u32, h: u32) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut buf, w, h);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(io::Error::other)?;
        writer.write_image_data(gray).map_err(io::Error::other)?;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erase_splits_stroke() {
        let mut ink = Ink::new();
        ink.start_stroke();
        for x in 0..100 {
            ink.add_point((x, 50, 3));
        }
        ink.erase_segment(50, 0, 50, 100, 10);
        assert_eq!(ink.strokes.len(), 2);
        assert!(ink.strokes[0].iter().all(|&(x, _, _)| x < 50));
        assert!(ink.strokes[1].iter().all(|&(x, _, _)| x > 50));
    }

    #[test]
    fn png_encodes() {
        let shadow = vec![255u8; (fb::WIDTH * fb::HEIGHT) as usize];
        let png = rasterize_page_png(&shadow, Rect::new(100, 100, 500, 500)).unwrap();
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
    }

    #[test]
    fn ordered_dissolve_follows_writing_order() {
        let mut shadow = vec![255u8; (fb::WIDTH * fb::HEIGHT) as usize];
        // Two "words" written in order: word 1 at x=100 (written first),
        // word 2 at x=1200 (written second), same line.
        for y in 400..500 {
            shadow[(y * fb::WIDTH + 100) as usize] = 0;
            shadow[(y * fb::WIDTH + 1200) as usize] = 0;
        }
        let mut ordered: Vec<Point> = Vec::new();
        for y in 400..500 {
            ordered.push((100, y, 2)); // first word written first
        }
        for y in 400..500 {
            ordered.push((1200, y, 2));
        }
        let d = Dissolve::plan_ordered(&shadow, &ordered, 26).unwrap();
        let max_stage_at = |x: i32| {
            d.pixels
                .iter()
                .filter(|(idx, _)| idx % fb::WIDTH as u32 == x as u32)
                .map(|&(_, st)| st)
                .max()
                .unwrap()
        };
        assert!(max_stage_at(100) < max_stage_at(1200));
        // last-written word fades last (its pixels take the earliest stroke
        // point covering them, so the max is just under effective-1, where
        // effective = stages - SOFT_FRONTIER)
        assert!(max_stage_at(1200) >= 12);
    }
}
