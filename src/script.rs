//! Handwriting synthesis.
//!
//! The reply is set in a script font at 96px (embedded Dancing Script, or
//! `$HORCRUX_FONT` if set), each wrapped line is rasterized to a binary
//! mask, the mask is thinned to a 1px skeleton (Zhang-Suen), and the
//! skeleton is traced into stroke paths that replay left-to-right like ink
//! being written.
//!
//! Determinism matters: replanning after more text arrives must reproduce
//! the already-drawn strokes exactly. The caller therefore sends each
//! sentence as its own paragraph (`\n`-separated): a new sentence always
//! starts a fresh line, earlier lines never reflow, and per-line jitter is
//! seeded by line index — so plans are prefix-stable.

use ab_glyph::{point, Font, FontArc, ScaleFont};
use std::sync::OnceLock;

const FONT_BYTES: &[u8] = include_bytes!("../fonts/DancingScript.ttf");

/// Reply font bytes: $HORCRUX_FONT (path to a .ttf) if set and readable,
/// otherwise the embedded Dancing Script. Resolved once per process.
fn font_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    if let Ok(path) = std::env::var("HORCRUX_FONT") {
        if !path.is_empty() {
            return BYTES.get_or_init(|| match std::fs::read(&path) {
                Ok(b) => {
                    crate::log!("reply font: {path}");
                    b
                }
                Err(e) => {
                    crate::log!("HORCRUX_FONT {path}: {e}; using embedded Dancing Script");
                    FONT_BYTES.to_vec()
                }
            });
        }
    }
    FONT_BYTES
}

/// Raster size of the script font.
pub const PX: f32 = 86.0;
/// Left/right page margins.
const MARGIN: i32 = 100;
/// Line pitch.
const LINE_H: i32 = 108;
/// Padding around each line mask so swashes can overflow the advance box.
const PAD: i32 = 24;

pub struct ReplyPlan {
    /// Strokes in replay order: lines top to bottom, and within each line
    /// sorted by minimum x (left-to-right). Absolute screen coordinates.
    pub strokes: Vec<Vec<(i32, i32)>>,
    pub block_top: i32,
}

/// Plan the handwriting for `text`. Pass the `block_top` of an earlier plan
/// of the same turn to keep appended text below what is already drawn.
pub fn plan_reply(text: &str, block_top: Option<i32>, seed: u64) -> Option<ReplyPlan> {
    let font = FontArc::try_from_slice(font_bytes()).ok()?;
    let max_w = crate::fb::WIDTH - 2 * MARGIN;

    let lines = wrap(&font, text, max_w as f32);
    if lines.is_empty() {
        return None;
    }

    let total_h = lines.len() as i32 * LINE_H;
    let top = block_top.unwrap_or_else(|| ((crate::fb::HEIGHT - total_h) / 3).max(60));

    let mut rng = seed;
    let mut strokes = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // per-line vertical jitter +/-3px (LCG)
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let jitter = ((rng >> 16) % 7) as i32 - 3;
        let line_y = top + i as i32 * LINE_H + jitter;
        if line_y > crate::fb::HEIGHT - 40 {
            break; // ran off the page — skip further lines
        }
        let mut line_strokes = trace_line(&font, line, line_y);
        line_strokes.sort_by_key(|s| s.iter().map(|p| p.0).min().unwrap_or(0));
        strokes.extend(line_strokes);
    }
    if strokes.is_empty() {
        return None;
    }
    Some(ReplyPlan {
        strokes,
        block_top: top,
    })
}

// --- layout ------------------------------------------------------------------

fn advance(font: &FontArc, s: &str) -> f32 {
    let scaled = font.as_scaled(PX);
    s.chars().map(|c| scaled.h_advance(font.glyph_id(c))).sum()
}

/// Greedy word wrap. Crucially append-only stable: adding text at the end
/// never changes the breaks of earlier lines.
fn wrap(font: &FontArc, text: &str, max_w: f32) -> Vec<String> {
    let space_w = advance(font, " ");
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0.0f32;
        for word in para.split_whitespace() {
            let ww = advance(font, word);
            if cur.is_empty() {
                if ww <= max_w {
                    cur = word.to_string();
                    cur_w = ww;
                } else {
                    let (mut heads, tail, tail_w) = break_word(font, word, max_w);
                    lines.append(&mut heads);
                    cur = tail;
                    cur_w = tail_w;
                }
            } else if cur_w + space_w + ww <= max_w {
                cur.push(' ');
                cur.push_str(word);
                cur_w += space_w + ww;
            } else {
                lines.push(std::mem::take(&mut cur));
                if ww <= max_w {
                    cur = word.to_string();
                    cur_w = ww;
                } else {
                    let (mut heads, tail, tail_w) = break_word(font, word, max_w);
                    lines.append(&mut heads);
                    cur = tail;
                    cur_w = tail_w;
                }
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines
}

/// Hard-break a single word wider than the text column.
fn break_word(font: &FontArc, word: &str, max_w: f32) -> (Vec<String>, String, f32) {
    let mut heads = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0.0f32;
    for c in word.chars() {
        let cw = advance(font, &c.to_string());
        if cur_w + cw > max_w && !cur.is_empty() {
            heads.push(std::mem::take(&mut cur));
            cur_w = 0.0;
        }
        cur.push(c);
        cur_w += cw;
    }
    (heads, cur, cur_w)
}

// --- per-line raster -> skeleton -> strokes -----------------------------------

/// Rasterize one line, thin it, trace it, and return strokes in absolute
/// screen coordinates. `line_y` is the line's typographic top on screen.
fn trace_line(font: &FontArc, line: &str, line_y: i32) -> Vec<Vec<(i32, i32)>> {
    let scaled = font.as_scaled(PX);
    let ascent = scaled.ascent();
    let line_w = advance(font, line);
    let mask_w = (line_w.ceil() as i32 + 2 * PAD).max(1);
    let mask_h = ((ascent - scaled.descent()).ceil() as i32 + 2 * PAD).max(1);
    let mut mask = vec![0u8; (mask_w * mask_h) as usize];

    // glyph coverage > 0.5 sets a mask pixel
    let mut pen_x = PAD as f32;
    let baseline = PAD as f32 + ascent;
    for c in line.chars() {
        let id = font.glyph_id(c);
        let glyph = id.with_scale_and_position(PX, point(pen_x, baseline));
        pen_x += scaled.h_advance(id);
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                if cov > 0.5 {
                    let x = bounds.min.x as i32 + gx as i32;
                    let y = bounds.min.y as i32 + gy as i32;
                    if x >= 0 && y >= 0 && x < mask_w && y < mask_h {
                        mask[(y * mask_w + x) as usize] = 1;
                    }
                }
            });
        }
    }

    zhang_suen_thin(&mut mask, mask_w, mask_h);
    let paths = trace_paths(&mask, mask_w, mask_h);

    let x_start = ((crate::fb::WIDTH as f32 - line_w) / 2.0).round() as i32;
    let (ox, oy) = (x_start - PAD, line_y - PAD);
    paths
        .into_iter()
        .map(|path| path.into_iter().map(|(x, y)| (x + ox, y + oy)).collect())
        .collect()
}

/// Classic Zhang-Suen thinning, two sub-iterations until stable.
fn zhang_suen_thin(mask: &mut [u8], w: i32, h: i32) {
    loop {
        let mut changed = false;
        for phase in 0..2 {
            let mut kill = Vec::new();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    if mask[(y * w + x) as usize] == 0 {
                        continue;
                    }
                    let p = |dx: i32, dy: i32| mask[((y + dy) * w + (x + dx)) as usize];
                    let (p2, p3, p4, p5) = (p(0, -1), p(1, -1), p(1, 0), p(1, 1));
                    let (p6, p7, p8, p9) = (p(0, 1), p(-1, 1), p(-1, 0), p(-1, -1));
                    let b = p2 + p3 + p4 + p5 + p6 + p7 + p8 + p9;
                    if !(2..=6).contains(&b) {
                        continue;
                    }
                    let seq = [p2, p3, p4, p5, p6, p7, p8, p9, p2];
                    let a = seq.windows(2).filter(|w| w[0] == 0 && w[1] == 1).count();
                    if a != 1 {
                        continue;
                    }
                    let one = |v: u8| v == 1;
                    let cond = if phase == 0 {
                        !(one(p2) && one(p4) && one(p6)) && !(one(p4) && one(p6) && one(p8))
                    } else {
                        !(one(p2) && one(p4) && one(p8)) && !(one(p2) && one(p6) && one(p8))
                    };
                    if cond {
                        kill.push((y * w + x) as usize);
                    }
                }
            }
            if !kill.is_empty() {
                changed = true;
                for idx in kill {
                    mask[idx] = 0;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

const NEIGHBORS: [(i32, i32); 8] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
];

/// Trace the skeleton into paths: endpoint-seeded (degree-1 pixels first,
/// row-major) greedy 8-neighbor walk with a global visited mask. Paths
/// shorter than 3 points are dropped.
fn trace_paths(mask: &[u8], w: i32, h: i32) -> Vec<Vec<(i32, i32)>> {
    let at = |x: i32, y: i32| -> u8 {
        if x >= 0 && y >= 0 && x < w && y < h {
            mask[(y * w + x) as usize]
        } else {
            0
        }
    };
    let degree = |x: i32, y: i32| -> u32 {
        NEIGHBORS
            .iter()
            .map(|&(dx, dy)| at(x + dx, y + dy) as u32)
            .sum()
    };

    let mut seeds: Vec<(i32, i32)> = Vec::new();
    for want_endpoints in [true, false] {
        for y in 0..h {
            for x in 0..w {
                if at(x, y) == 1 && (degree(x, y) == 1) == want_endpoints {
                    seeds.push((x, y));
                }
            }
        }
    }

    let mut visited = vec![false; (w * h) as usize];
    let mut paths = Vec::new();
    for (sx, sy) in seeds {
        if visited[(sy * w + sx) as usize] {
            continue;
        }
        let mut path = vec![(sx, sy)];
        visited[(sy * w + sx) as usize] = true;
        let (mut cx, mut cy) = (sx, sy);
        loop {
            let mut next = None;
            for &(dx, dy) in &NEIGHBORS {
                let (nx, ny) = (cx + dx, cy + dy);
                if at(nx, ny) == 1 && !visited[(ny * w + nx) as usize] {
                    next = Some((nx, ny));
                    break;
                }
            }
            match next {
                Some((nx, ny)) => {
                    visited[(ny * w + nx) as usize] = true;
                    path.push((nx, ny));
                    cx = nx;
                    cy = ny;
                }
                None => break,
            }
        }
        if path.len() >= 3 {
            paths.push(path);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_loads_and_plans() {
        let plan = plan_reply("Hello, Tom. It is me.", None, 42).unwrap();
        assert!(!plan.strokes.is_empty());
        assert!(plan.strokes.iter().map(Vec::len).sum::<usize>() > 100);
        // all strokes inside the screen
        for s in &plan.strokes {
            for &(x, y) in s {
                assert!((0..crate::fb::WIDTH).contains(&x), "x={x}");
                assert!((0..crate::fb::HEIGHT).contains(&y), "y={y}");
            }
        }
    }

    #[test]
    fn replan_is_prefix_stable() {
        // caller contract: sentences are paragraphs, so appending one can
        // only add lines, never reflow existing ones
        let a = plan_reply("One sentence here.", None, 7).unwrap();
        let b = plan_reply("One sentence here.\nAnd another one.", Some(a.block_top), 7).unwrap();
        assert!(b.strokes.len() > a.strokes.len());
        for (sa, sb) in a.strokes.iter().zip(b.strokes.iter()) {
            assert_eq!(sa, sb);
        }
    }

    #[test]
    fn wrap_respects_width() {
        let font = FontArc::try_from_slice(FONT_BYTES).unwrap();
        let max_w = (crate::fb::WIDTH - 2 * MARGIN) as f32;
        let long = "word ".repeat(200);
        for line in wrap(&font, long.trim(), max_w) {
            assert!(advance(&font, &line) <= max_w + 1.0, "'{line}' too wide");
        }
    }
}
