//! Host-side dev tool: render the handwriting plan of a sample reply to a
//! PNG so the skeletonized strokes can be eyeballed without a device.
//!
//!     cargo run --example render_sample -- /tmp/horcrux-sample.png

use std::path::Path;

#[path = "../src/fb.rs"]
#[allow(dead_code)]
mod fb;
#[path = "../src/macros.rs"]
#[allow(unused)]
mod macros;
#[path = "../src/script.rs"]
#[allow(dead_code)]
mod script;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/horcrux-sample.png".to_string());
    let text = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "Hello, Tom. I found your diary.\nWho are you, really?".to_string());
    let plan = script::plan_reply(&text, 0x5EED).expect("plan failed");
    let point_count: usize = plan.strokes.iter().map(Vec::len).sum();
    println!(
        "{} strokes, {} points, block_top {}, {} lines at {:.0}px",
        plan.strokes.len(),
        point_count,
        plan.block_top,
        plan.line_count,
        plan.font_px
    );

    let (w, h) = (fb::WIDTH as usize, fb::HEIGHT as usize);
    let mut img = vec![255u8; w * h];
    let mut stamp = |x: i32, y: i32, r: i32| {
        for dy in -r..=r {
            let half = ((r * r - dy * dy) as f32).sqrt() as i32;
            for dx in -half..=half {
                let (px, py) = (x + dx, y + dy);
                if px >= 0 && py >= 0 && px < w as i32 && py < h as i32 {
                    img[py as usize * w + px as usize] = 0;
                }
            }
        }
    };
    for stroke in &plan.strokes {
        let mut prev: Option<(i32, i32)> = None;
        for &(x, y) in stroke {
            if let Some((lx, ly)) = prev {
                let steps = (x - lx).abs().max((y - ly).abs()).max(1);
                for i in 0..=steps {
                    let t = i as f32 / steps as f32;
                    stamp(
                        (lx as f32 + (x - lx) as f32 * t).round() as i32,
                        (ly as f32 + (y - ly) as f32 * t).round() as i32,
                        1,
                    );
                }
            } else {
                stamp(x, y, 1);
            }
            prev = Some((x, y));
        }
    }

    let file = std::fs::File::create(Path::new(&out)).unwrap();
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(&img).unwrap();
    println!("wrote {out}");
}
