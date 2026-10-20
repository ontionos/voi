//! The drawing both Windows widgets share, ported from the Mac's: the row of
//! meter bars (`mac/meter.rs`) and the light that travels around the glass
//! while it is listening (`mac/sweep.rs`). The same constants, the same
//! shapes; egui meshes instead of Core Animation layers.

use eframe::egui::{self, Color32, CornerRadius, Mesh, Pos2, Rect, Stroke, StrokeKind, Vec2};

/// Cascadia Mono is narrower than SF Mono (0.586 of an em to a character
/// against 0.618), so it is set this much larger: the same characters to a
/// line, and lines that wrap at the same words as on the Mac.
pub const MONO_SCALE: f32 = 1.055;

/// Whether the screenshots are being taken: the blur window is not in a
/// picture read back from the drawn window, so its tint is painted in.
pub static SHOT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The blur window's dark wash, painted into a screenshot in its place.
pub fn shot_backdrop(painter: &egui::Painter, glass: Rect, corner: f32, alpha: f32) {
    if SHOT.load(std::sync::atomic::Ordering::Relaxed) {
        painter.rect_filled(glass, CornerRadius::same(corner.round() as u8), Color32::from_rgba_unmultiplied(28, 28, 30, (158.0 * alpha) as u8));
    }
}

/// The recording red, one definition for everything that turns red.
pub const RED: (f32, f32, f32) = (1.0, 0.25, 0.22);

/// sRGB components and an alpha, as the Mac writes its colours.
pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
        (a.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// The meter's colour: white at rest, exactly [`RED`] while recording.
pub fn meter_color(tint: f32, alpha: f32) -> Color32 {
    let mix = |white: f32, red: f32| white + (red - white) * tint;
    rgba(mix(1.0, RED.0), mix(1.0, RED.1), mix(1.0, RED.2), alpha)
}

/// How much of a bar's height survives at `r`, its distance from the centre
/// of the row as a fraction of the way to the end (`mac/meter.rs`).
pub fn ends_taper(r: f32) -> f32 {
    const SHOULDER: f32 = 0.55;
    const ENDS: f32 = 0.12;
    if r <= SHOULDER {
        return 1.0;
    }
    let t = ((r - SHOULDER) / (1.0 - SHOULDER)).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    1.0 - (1.0 - ENDS) * s
}

/// The row between `left` and `right`, centred on `mid`, at most `box_h`
/// tall: `mac::meter::paint`, line for line.
pub fn meter(painter: &egui::Painter, left: f32, right: f32, mid: f32, box_h: f32, wave: &[f32], tint: f32, fade: f32) {
    let width = (right - left).max(1.0);
    let room = ((width - 45.0) / 200.0).clamp(0.0, 1.0);
    let mut pitch = 1.8 + (2.1 - 1.8) * room;
    let fill = 0.58 + (0.62 - 0.58) * room;
    // Where a point is a pixel or less, bars thinner than two pixels fall
    // between pixels and the row breaks into uneven clumps. There the row is
    // laid on whole pixels — a two-pixel pitch, one-pixel bars — which is as
    // close as such a screen comes to the Mac's 2.1-point rhythm.
    let ppp = painter.ctx().pixels_per_point();
    let coarse = ppp < 1.5;
    if coarse {
        pitch = (pitch * ppp).round().max(2.0) / ppp;
    }
    let slots = ((width / pitch) as usize).max(3) | 1;
    let last = (wave.len() - 1) as f32;
    let span = last / (slots - 1) as f32;
    let levels: Vec<f32> = (0..slots)
        .map(|i| {
            let window = span * 1.6;
            let centre = i as f32 * span;
            let from = (centre - window / 2.0).max(0.0).round() as usize;
            let to = ((centre + window / 2.0).min(last).round() as usize).max(from);
            let taken = &wave[from..=to];
            taken.iter().sum::<f32>() / taken.len() as f32
        })
        .collect();
    let (step, bar_w, x0) = if coarse {
        let step = pitch;
        let bar_w = ((step * fill * ppp).floor().max(1.0)) / ppp;
        let used = step * (slots as f32 - 1.0) + bar_w;
        let x0 = ((left + (width - used) / 2.0) * ppp).round() / ppp;
        (step, bar_w, x0)
    } else {
        let step = width / slots as f32;
        let bar_w = (step * fill).clamp(0.9, 2.4);
        (step, bar_w, left + (step - bar_w) / 2.0)
    };
    let middle = (levels.len() - 1) as f32 / 2.0;
    let color = meter_color(tint, fade);
    for (i, level) in levels.iter().enumerate() {
        let taper = ends_taper((i as f32 - middle).abs() / middle);
        let h = (box_h * level * taper * fade).max(1.0);
        let rect = Rect::from_min_size(Pos2::new(x0 + i as f32 * step, mid - h / 2.0), Vec2::new(bar_w, h));
        let radius = bar_w.min(h) / 2.0;
        painter.rect_filled(rect, CornerRadius::same(radius.round() as u8), color);
    }
}

/// The glass's hairline: inset half a point, the corner of the glass.
pub fn hairline(painter: &egui::Painter, glass: Rect, corner: f32, alpha: f32) {
    painter.rect_stroke(
        glass.shrink(0.5),
        CornerRadius::same(corner.round() as u8),
        Stroke::new(1.0, rgba(1.0, 1.0, 1.0, alpha)),
        StrokeKind::Middle,
    );
}

/// How far the glow reaches across the border, in and out.
pub const REACH: f32 = 20.0;
/// Width of the bright line along the border.
const CORE: f32 = 1.4;
/// Gaussian falloff of the glow.
const FALLOFF: f32 = 4.0;
/// Strokes the Mac paints the glow's mask with.
const STEPS: usize = 56;
/// The glow pass's share of the light.
const GLOW_ALPHA: f32 = 0.30;
/// The lit stretch, as a share of the perimeter.
const SPAN: f32 = 0.60;
/// The blob's radial gradient: (position from the centre, alpha).
const STOPS: [(f32, f32); 5] = [(0.0, 1.0), (0.25, 0.74), (0.50, 0.32), (0.75, 0.07), (1.0, 0.0)];

/// The light travelling around the glass while it listens.
#[derive(Default)]
pub struct Sweep {
    /// Where the blob is, as a share of one lap.
    phase: f32,
    /// How lit it is, easing in and out over 0.35 s.
    pub alpha: f32,
}

/// The rounded rectangle's perimeter.
fn perimeter(rect: Rect, r: f32) -> f32 {
    let r = r.min(rect.width() / 2.0).min(rect.height() / 2.0);
    2.0 * (rect.width() - 2.0 * r) + 2.0 * (rect.height() - 2.0 * r) + std::f32::consts::TAU * r
}

/// A point `s` along the outline (clockwise from the top-left end of the top
/// edge, y down), with its outward normal.
fn along(rect: Rect, r: f32, s: f32) -> (Pos2, Vec2) {
    let r = r.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let (w, h) = (rect.width() - 2.0 * r, rect.height() - 2.0 * r);
    let quarter = std::f32::consts::FRAC_PI_2 * r;
    let total = 2.0 * w + 2.0 * h + 4.0 * quarter;
    let mut s = s.rem_euclid(total);
    let corner = |c: Pos2, a0: f32, s: f32| {
        let a = a0 + if r > 0.0 { s / r } else { 0.0 };
        let n = Vec2::new(a.cos(), a.sin());
        (c + n * r, n)
    };
    let (l, t, rt, b) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    use std::f32::consts::{FRAC_PI_2, PI};
    if s < w {
        return (Pos2::new(l + r + s, t), Vec2::new(0.0, -1.0));
    }
    s -= w;
    if s < quarter {
        return corner(Pos2::new(rt - r, t + r), -FRAC_PI_2, s);
    }
    s -= quarter;
    if s < h {
        return (Pos2::new(rt, t + r + s), Vec2::new(1.0, 0.0));
    }
    s -= h;
    if s < quarter {
        return corner(Pos2::new(rt - r, b - r), 0.0, s);
    }
    s -= quarter;
    if s < w {
        return (Pos2::new(rt - r - s, b), Vec2::new(0.0, 1.0));
    }
    s -= w;
    if s < quarter {
        return corner(Pos2::new(l + r, b - r), FRAC_PI_2, s);
    }
    s -= quarter;
    if s < h {
        return (Pos2::new(l, b - r - s), Vec2::new(-1.0, 0.0));
    }
    s -= h;
    corner(Pos2::new(l + r, t + r), PI, s.min(quarter))
}

/// The blob's strength at `d` from its centre, for a blob `diameter` wide.
fn blob(d: f32, diameter: f32) -> f32 {
    let x = (d / (diameter / 2.0)).clamp(0.0, 1.0);
    for pair in STOPS.windows(2) {
        let ((p0, a0), (p1, a1)) = (pair[0], pair[1]);
        if x <= p1 {
            return a0 + (a1 - a0) * (x - p0) / (p1 - p0);
        }
    }
    0.0
}

/// The glow's mask across the border, at `x` from the line (either side):
/// the Mac's 56 strokes, each `2r + CORE` wide, laid over one another.
fn glow_profile(x: f32) -> f32 {
    let mut clear = 1.0;
    for i in 0..STEPS {
        let r = REACH * (1.0 - i as f32 / (STEPS - 1) as f32);
        if x.abs() <= r + CORE / 2.0 {
            clear *= 1.0 - 0.17 * (-(r / FALLOFF).powi(2)).exp();
        }
    }
    1.0 - clear
}

impl Sweep {
    /// Advance by `dt`, easing toward lit or not, with the lap paced by the
    /// size of the outline against `reference`, a perimeter.
    pub fn tick(&mut self, track: Rect, corner: f32, reference: f32, lit: bool, dt: f32) {
        let length = perimeter(track, corner);
        let lap = 3.4 * (length / reference).max(1.0).powf(0.35);
        self.phase = (self.phase + dt / lap).rem_euclid(1.0);
        let wanted = if lit { 1.0 } else { 0.0 };
        self.alpha += (wanted - self.alpha) * (1.0 - (-dt / 0.35).exp());
        if (self.alpha - wanted).abs() < 0.004 {
            self.alpha = wanted;
        }
    }

    /// Paint it around `track`.
    pub fn paint(&self, painter: &egui::Painter, track: Rect, corner: f32) {
        if self.alpha <= 0.0 {
            return;
        }
        let length = perimeter(track, corner);
        let diameter = length * SPAN;
        let (head, _) = along(track, corner, self.phase * length);
        // Sample the outline every couple of points, over the lit stretch only.
        let step = 2.0;
        let samples = (length / step).ceil() as usize;
        let points: Vec<(Pos2, Vec2, f32)> = (0..=samples)
            .map(|i| {
                let (p, n) = along(track, corner, i as f32 * length / samples as f32);
                (p, n, blob((p - head).length(), diameter))
            })
            .collect();
        let red = (1.0, 0.30, 0.24);
        let mut mesh = Mesh::default();
        // The glow: bands across the border, strongest on the line.
        let bands = 20;
        for k in 0..bands {
            let x0 = -REACH + k as f32 * (2.0 * REACH / bands as f32);
            let x1 = x0 + 2.0 * REACH / bands as f32;
            let a = glow_profile((x0 + x1) / 2.0) * GLOW_ALPHA * self.alpha;
            strip(&mut mesh, &points, x0, x1, |strength| rgba(red.0, red.1, red.2, a * strength));
        }
        // The line itself.
        strip(&mut mesh, &points, -CORE / 2.0, CORE / 2.0, |strength| {
            rgba(red.0, red.1, red.2, self.alpha * strength)
        });
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// One band of the outline from `x0` to `x1` across it, coloured by each
/// point's blob strength.
fn strip(mesh: &mut Mesh, points: &[(Pos2, Vec2, f32)], x0: f32, x1: f32, color: impl Fn(f32) -> Color32) {
    let base = mesh.vertices.len() as u32;
    for (p, n, strength) in points {
        let c = color(*strength);
        mesh.colored_vertex(*p + *n * x0, c);
        mesh.colored_vertex(*p + *n * x1, c);
    }
    for i in 0..points.len().saturating_sub(1) as u32 {
        let a = base + 2 * i;
        if points[i as usize].2 <= 0.0 && points[i as usize + 1].2 <= 0.0 {
            continue;
        }
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outline_closes_on_itself() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(368.0, 93.0));
        let length = perimeter(rect, 13.0);
        let (start, _) = along(rect, 13.0, 0.0);
        let (end, _) = along(rect, 13.0, length - 0.001);
        assert!((start - end).length() < 0.1, "{start:?} vs {end:?}");
        // Every point lies on the rectangle's edge band.
        for i in 0..200 {
            let (p, _) = along(rect, 13.0, i as f32 * length / 200.0);
            assert!(rect.expand(0.01).contains(p), "{p:?} off the outline");
        }
    }

    #[test]
    fn the_blob_fades_to_nothing_at_its_edge() {
        assert_eq!(blob(0.0, 100.0), 1.0);
        assert!((blob(25.0, 100.0) - 0.32).abs() < 1e-5, "half way out is the 0.50 stop");
        assert_eq!(blob(60.0, 100.0), 0.0);
    }

    #[test]
    fn the_glow_is_strongest_on_the_line_and_gone_at_its_reach() {
        assert!(glow_profile(0.0) > glow_profile(4.0));
        assert!(glow_profile(4.0) > glow_profile(12.0));
        assert!(glow_profile(REACH + 2.0) < 1e-6);
    }

    #[test]
    fn the_last_bar_is_a_fraction_of_the_middle() {
        assert_eq!(ends_taper(0.0), 1.0);
        assert!(ends_taper(1.0) < 0.2);
    }
}
