//! Lay the Mac's and Windows' pictures of each state side by side, and say
//! how far apart they are.
//!
//!   parity compare MAC_DIR WIN_DIR OUT_DIR   # every name found in both
//!   parity sample FILE X Y                   # one pixel, as R G B A
//!
//! Both are brought to the same scale (the Mac draws at 2×), laid on the
//! same desktop grey, and compared pixel by pixel. For each pair it writes
//! `OUT_DIR/NAME.png` — Mac, Windows, and a difference map where the brighter
//! a pixel the further apart the two are — and a line in `OUT_DIR/report.md`:
//! the mean difference, the share of pixels that differ visibly, and where the
//! drawn content sits in each, so a shift of a few points shows as a number.

use std::path::{Path, PathBuf};

/// The desktop the translucent windows are laid on, the same for both.
const DESK: [u8; 3] = [58, 64, 76];
/// A pixel counts as different past this, out of 255, on any channel.
const VISIBLE: i32 = 24;

struct Image {
    w: usize,
    h: usize,
    /// RGBA, straight alpha.
    px: Vec<u8>,
}

fn load(path: &Path) -> Image {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).expect("open")));
    let mut reader = decoder.read_info().expect("png");
    let mut buf = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut buf).expect("frame");
    let (w, h) = (info.width as usize, info.height as usize);
    let px = match info.color_type {
        png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
        png::ColorType::Rgb => buf[..w * h * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        other => panic!("{}: unsupported colour type {other:?}", path.display()),
    };
    Image { w, h, px }
}

fn save(path: &Path, w: usize, h: usize, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().expect("header").write_image_data(rgba).expect("data");
}

/// Shrunk by a whole factor, averaging (premultiplied, so a transparent
/// neighbour does not darken an edge).
fn shrink(img: &Image, f: usize) -> Image {
    if f <= 1 {
        return Image { w: img.w, h: img.h, px: img.px.clone() };
    }
    let (w, h) = (img.w / f, img.h / f);
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 4];
            for dy in 0..f {
                for dx in 0..f {
                    let i = ((y * f + dy) * img.w + x * f + dx) * 4;
                    let a = img.px[i + 3] as f32 / 255.0;
                    for c in 0..3 {
                        acc[c] += img.px[i + c] as f32 * a;
                    }
                    acc[3] += a;
                }
            }
            let n = (f * f) as f32;
            let o = (y * w + x) * 4;
            let a = acc[3] / n;
            for c in 0..3 {
                px[o + c] = if acc[3] > 0.0 { (acc[c] / acc[3]).round() as u8 } else { 0 };
            }
            px[o + 3] = (a * 255.0).round() as u8;
        }
    }
    Image { w, h, px }
}

/// Laid on the desktop: opaque from here on.
fn flatten(img: &Image) -> Vec<[u8; 3]> {
    img.px
        .chunks(4)
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            let mix = |c: u8, d: u8| (c as f32 * a + d as f32 * (1.0 - a)).round() as u8;
            [mix(p[0], DESK[0]), mix(p[1], DESK[1]), mix(p[2], DESK[2])]
        })
        .collect()
}

/// Where anything is drawn: the box around every pixel that is not desktop.
fn content_box(px: &[[u8; 3]], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let mut b: Option<(usize, usize, usize, usize)> = None;
    for y in 0..h {
        for x in 0..w {
            let p = px[y * w + x];
            if (0..3).any(|c| (p[c] as i32 - DESK[c] as i32).abs() > 6) {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    b
}

fn compare(mac: &Path, win: &Path, out: &Path) {
    std::fs::create_dir_all(out).expect("out dir");
    let mut names: Vec<String> = std::fs::read_dir(mac)
        .expect("mac dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".png") && win.join(n).exists())
        .collect();
    names.sort();
    let mut report = String::from("| State | Mac (pt) | Windows (pt) | Content box Mac | Content box Windows | Mean diff | Pixels differing |\n|---|---|---|---|---|---|---|\n");
    for name in names {
        let m = load(&mac.join(&name));
        let w = load(&win.join(&name));
        // The Mac draws at twice the points; bring it to Windows' scale.
        let factor = (m.w as f32 / w.w as f32).round().max(1.0) as usize;
        let m = shrink(&m, factor);
        let (cw, ch) = (m.w.max(w.w), m.h.max(w.h));
        let fm = flatten(&m);
        let fw = flatten(&w);
        let at = |px: &[[u8; 3]], iw: usize, ih: usize, x: usize, y: usize| if x < iw && y < ih { px[y * iw + x] } else { DESK };
        let gap = 12;
        let total_w = cw * 3 + gap * 2;
        let mut canvas = vec![0u8; total_w * ch * 4];
        let (mut sum, mut differ) = (0f64, 0usize);
        for y in 0..ch {
            for x in 0..cw {
                let a = at(&fm, m.w, m.h, x, y);
                let b = at(&fw, w.w, w.h, x, y);
                let d = (0..3).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap();
                sum += d as f64;
                if d > VISIBLE {
                    differ += 1;
                }
                let put = |canvas: &mut Vec<u8>, ox: usize, p: [u8; 3]| {
                    let i = (y * total_w + ox + x) * 4;
                    canvas[i..i + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
                };
                put(&mut canvas, 0, a);
                put(&mut canvas, cw + gap, b);
                let heat = (d * 3).min(255) as u8;
                put(&mut canvas, 2 * (cw + gap), [heat, heat / 3, heat / 3]);
            }
        }
        save(&out.join(&name), total_w, ch, &canvas);
        let n = (cw * ch) as f64;
        let boxed = |b: Option<(usize, usize, usize, usize)>| match b {
            Some((x0, y0, x1, y1)) => format!("{x0},{y0} – {x1},{y1}"),
            None => "empty".into(),
        };
        report.push_str(&format!(
            "| {} | {}×{} | {}×{} | {} | {} | {:.1} | {:.1}% |\n",
            name.trim_end_matches(".png"),
            m.w,
            m.h,
            w.w,
            w.h,
            boxed(content_box(&fm, m.w, m.h)),
            boxed(content_box(&fw, w.w, w.h)),
            sum / n,
            100.0 * differ as f64 / n
        ));
    }
    std::fs::write(out.join("report.md"), &report).expect("report");
    print!("{report}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("compare") if args.len() == 4 => compare(&PathBuf::from(&args[1]), &PathBuf::from(&args[2]), &PathBuf::from(&args[3])),
        Some("sample") if args.len() == 4 => {
            let img = load(Path::new(&args[1]));
            let (x, y): (usize, usize) = (args[2].parse().unwrap(), args[3].parse().unwrap());
            let i = (y * img.w + x) * 4;
            println!("{} {} {} {}", img.px[i], img.px[i + 1], img.px[i + 2], img.px[i + 3]);
        }
        _ => {
            eprintln!("usage: parity compare MAC_DIR WIN_DIR OUT_DIR | parity sample FILE X Y");
            std::process::exit(2);
        }
    }
}
