//! Rounded-rectangle border ring computation shared by the winit and DRM backends.
//!

use smithay::utils::{Logical, Rectangle};
use std::f32::consts::TAU;
use std::sync::OnceLock;
use std::time::Instant;

/// Animated border effect names (`decorations { border_effect ... }`):
/// `none` (default), `breathing`, `rainbow`, `rainbow_solid`, `gradient`,
pub(crate) fn is_animated_border_effect(effect: &str) -> bool {
    if effect == "bg" || effect.starts_with("bg-") {
        return bg_effect_count(effect).is_some();
    }
    matches!(
        effect,
        "breathing"
            | "rainbow"
            | "rainbow_solid"
            | "gradient"
            | "flow"
            | "glow"
            | "pulse"
            | "ember"
            | "chase"
    )
}

/// Palette size for the wallpaper `bg` border effect: `bg` means 4 colors,
/// `bg-N` means N (clamped to 2..=8). `None` for anything else, so a typo
pub(crate) fn bg_effect_count(effect: &str) -> Option<usize> {
    if effect == "bg" {
        return Some(4);
    }
    effect
        .strip_prefix("bg-")
        .and_then(|n| n.parse::<usize>().ok())
        .map(|n| n.clamp(2, 8))
}

/// Angular position (0..1) of `point` around `center`, for the positional
/// border effects. Each ring fragment samples the effect at its own angle so
pub(crate) fn ring_angle(point: (f32, f32), center: (f32, f32)) -> f32 {
    ((point.1 - center.1).atan2(point.0 - center.0) / TAU).rem_euclid(1.0)
}

/// Shared clock for animated border effects: seconds since first use,
/// scaled by the configured effect speed. A single process-wide epoch so
static EFFECT_EPOCH: OnceLock<Instant> = OnceLock::new();

pub(crate) fn border_effect_clock(speed: f32) -> f32 {
    EFFECT_EPOCH
        .get_or_init(Instant::now)
        .elapsed()
        .as_secs_f32()
        * speed.max(0.0)
}

/// Tint one border-ring fragment for the focused window.
///
pub(crate) fn border_effect_color(
    base: [f32; 4],
    effect: &str,
    t: f32,
    angle: f32,
    grad: [f32; 4],
) -> [f32; 4] {
    if let Some(count) = bg_effect_count(effect) {
        return border_bg_color(base, count, t, angle);
    }
    match effect {
        "breathing" => {
            let p = 0.35 + 0.65 * (0.5 + 0.5 * (t / 2.5 * TAU).sin());
            [base[0] * p, base[1] * p, base[2] * p, base[3]]
        }
        "rainbow" => {
            let (r, g, b) = hsv_to_rgb((t / 6.0 + angle).rem_euclid(1.0), 0.75, 1.0);
            [r, g, b, base[3]]
        }
        "gradient" => {
            let m = 0.5 + 0.5 * ((t / 4.0 + angle) * TAU).sin();
            [
                base[0] + (grad[0] - base[0]) * m,
                base[1] + (grad[1] - base[1]) * m,
                base[2] + (grad[2] - base[2]) * m,
                base[3],
            ]
        }
        "flow" => {
            let d = (angle - t / 2.0).rem_euclid(1.0);
            let band = (-(d * d) / 0.02).exp();
            let k = (band * 0.9).min(1.0);
            [
                base[0] + (1.0 - base[0]) * k,
                base[1] + (1.0 - base[1]) * k,
                base[2] + (1.0 - base[2]) * k,
                base[3],
            ]
        }
        "glow" => {
            let p = 0.5 + 0.5 * (t / 2.0 * TAU).sin();
            let boost = 0.7 + 0.45 * p;
            let k = 0.45 * p;
            [
                (base[0] * boost + k).min(1.0),
                (base[1] * boost + k).min(1.0),
                (base[2] * boost + k).min(1.0),
                base[3],
            ]
        }
        "pulse" => {
            let d = (angle - t / 1.2).rem_euclid(1.0);
            let band = (-(d * d) / 0.004).exp();
            let k = band.min(1.0);
            [
                base[0] + (1.0 - base[0]) * k,
                base[1] + (1.0 - base[1]) * k,
                base[2] + (1.0 - base[2]) * k,
                base[3],
            ]
        }
        "ember" => {
            let f = 0.72 + 0.17 * (t * 11.0 + angle * 12.0).sin() + 0.11 * (t * 5.7 + 1.7).sin();
            let tip = ((f - 0.86) / 0.14).clamp(0.0, 1.0);
            let k = tip * tip * 0.8;
            [
                (base[0] * f + (1.0 - base[0]) * k).min(1.0),
                (base[1] * f + (1.0 - base[1]) * k).min(1.0),
                (base[2] * f + (1.0 - base[2]) * k).min(1.0),
                base[3],
            ]
        }
        "rainbow_solid" => {
            let (r, g, b) = hsv_to_rgb((t / 6.0).rem_euclid(1.0), 0.75, 1.0);
            [r, g, b, base[3]]
        }
        "chase" => {
            let head = (t / 3.0).rem_euclid(1.0);
            let d1 = (angle - head).rem_euclid(1.0);
            let d1 = d1.min(1.0 - d1);
            let d2 = (angle - head - 0.5).rem_euclid(1.0);
            let d2 = d2.min(1.0 - d2);
            let b1 = (-(d1 * d1) / 0.006).exp();
            let b2 = (-(d2 * d2) / 0.006).exp();
            let dim = 0.3 * (0.85 + 0.15 * (t / 2.5 * TAU).sin());
            [
                (base[0] * dim + base[0] * b1 + grad[0] * b2).min(1.0),
                (base[1] * dim + base[1] * b1 + grad[1] * b2).min(1.0),
                (base[2] * dim + base[2] * b1 + grad[2] * b2).min(1.0),
                base[3],
            ]
        }
        _ => base,
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 1.0 {
        (c, x, 0.0)
    } else if h < 2.0 {
        (x, c, 0.0)
    } else if h < 3.0 {
        (0.0, c, x)
    } else if h < 4.0 {
        (0.0, x, c)
    } else if h < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (r + m, g + m, b + m)
}

/// Dominant wallpaper colors for the `bg` border effects, sampled once from
/// the decoded wallpaper by whichever backend loads it first. The wallpaper
static BORDER_PALETTE: OnceLock<Vec<[f32; 4]>> = OnceLock::new();

/// Remember the wallpaper's dominant colors (up to 8) for the `bg` effects.
/// Only the first call wins; later wallpapers never reload anyway.
pub(crate) fn note_wallpaper_colors(img: &image::RgbaImage) {
    BORDER_PALETTE.get_or_init(|| wallpaper_palette(img, 8));
}

fn border_palette() -> &'static [[f32; 4]] {
    BORDER_PALETTE.get().map(Vec::as_slice).unwrap_or(&[])
}

/// Wallpaper palette runner: `count` dominant colors orbit the ring (~4s
/// lap) with smooth blends between stops. Falls back to `base` when the
fn border_bg_color(base: [f32; 4], count: usize, t: f32, angle: f32) -> [f32; 4] {
    let pal = border_palette();
    let n = count.min(pal.len());
    if n < 2 {
        return base;
    }
    let n_f = n as f32;
    let x = (angle * n_f + t / 4.0 * n_f).rem_euclid(n_f);
    let i0 = x.floor() as usize % n;
    let i1 = (i0 + 1) % n;
    let f = x.fract();
    let smooth = f * f * (3.0 - 2.0 * f);
    let a = pal[i0];
    let b = pal[i1];
    [
        a[0] + (b[0] - a[0]) * smooth,
        a[1] + (b[1] - a[1]) * smooth,
        a[2] + (b[2] - a[2]) * smooth,
        base[3],
    ]
}

/// Extract up to `max` dominant colors from a wallpaper: stride-sample the
/// pixels, histogram into 12-bit buckets, then greedily take the fullest
pub(crate) fn wallpaper_palette(img: &image::RgbaImage, max: usize) -> Vec<[f32; 4]> {
    use std::collections::HashMap;

    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 || max == 0 {
        return Vec::new();
    }
    let step = ((w * h / 20_000) as f64).sqrt().round() as usize;
    let step = step.max(1);
    let mut hist: HashMap<u16, u32> = HashMap::new();
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let p = img.get_pixel(x as u32, y as u32);
            let bucket =
                ((p[0] as u16 >> 4) << 8) | ((p[1] as u16 >> 4) << 4) | (p[2] as u16 >> 4);
            *hist.entry(bucket).or_insert(0) += 1;
        }
    }
    let mut buckets: Vec<(u16, u32)> = hist.into_iter().collect();
    buckets.sort_by(|a, b| b.1.cmp(&a.1));

    let mut picked: Vec<(u16, f32, f32)> = Vec::new();
    for min_hue in [25.0f32, 0.0] {
        for (bucket, _) in &buckets {
            if picked.len() >= max {
                break;
            }
            if picked.iter().any(|(b, _, _)| b == bucket) {
                continue;
            }
            let [r, g, b, _] = bucket_color(*bucket);
            let hue = rgb_hue(r, g, b);
            let sat = rgb_saturation(r, g, b);
            if sat < 0.08 {
                if picked.iter().any(|(_, _, s)| *s < 0.08) {
                    continue;
                }
            } else if picked
                .iter()
                .any(|(_, h, s)| *s >= 0.08 && hue_distance(hue, *h) < min_hue)
            {
                continue;
            }
            picked.push((*bucket, hue, sat));
        }
    }
    picked
        .into_iter()
        .map(|(bucket, _, _)| bucket_color(bucket))
        .collect()
}

/// Center color of a 12-bit histogram bucket (opaque).
fn bucket_color(bucket: u16) -> [f32; 4] {
    let r = (((bucket >> 8) & 0xf) * 16 + 8) as f32 / 255.0;
    let g = (((bucket >> 4) & 0xf) * 16 + 8) as f32 / 255.0;
    let b = ((bucket & 0xf) * 16 + 8) as f32 / 255.0;
    [r, g, b, 1.0]
}

fn rgb_hue(r: f32, g: f32, b: f32) -> f32 {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d < 1e-6 {
        return 0.0;
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
}

fn rgb_saturation(r: f32, g: f32, b: f32) -> f32 {
    let max = r.max(g).max(b);
    if max < 1e-6 {
        return 0.0;
    }
    (max - r.min(g).min(b)) / max
}

/// Circular distance of two hue angles in degrees.
fn hue_distance(a: f32, b: f32) -> f32 {
    let d = (a - b).abs().rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Decompose the border ring around `window_rect` into logical rectangles.
pub(crate) fn border_ring_rects(
    window_rect: Rectangle<i32, Logical>,
    border_width: i32,
    rounding: i32,
) -> Vec<Rectangle<i32, Logical>> {
    let bw = border_width.max(1);
    let r = rounding.max(0);

    if r == 0 {
        return plain_border_ring_rects(window_rect, bw);
    }

    let outer = Rectangle::new(
        (window_rect.loc.x - bw, window_rect.loc.y - bw).into(),
        (window_rect.size.w + 2 * bw, window_rect.size.h + 2 * bw).into(),
    );
    let inner = window_rect;
    let radius_outer = r + bw;

    let mut rows: Vec<(i32, (i32, i32))> = Vec::new();

    for y in outer.loc.y..(outer.loc.y + outer.size.h) {
        let os = rounded_rect_row_spans(outer, radius_outer, y);
        let is = rounded_rect_row_spans(inner, r, y);

        for (oa, ob) in os {
            let mut running = oa;
            for (ia, ib) in &is {
                if *ib < running || *ia > ob {
                    continue;
                }
                if running < *ia {
                    rows.push((y, (running, *ia - 1)));
                }
                running = running.max(*ib + 1);
            }
            if running <= ob {
                rows.push((y, (running, ob)));
            }
        }
    }

    let mut rects: Vec<Rectangle<i32, Logical>> = Vec::new();
    for (y, (x0, x1)) in rows {
        match rects.iter_mut().find(|last| {
            last.loc.y + last.size.h == y
                && last.loc.x == x0
                && last.loc.x + last.size.w - 1 == x1
        }) {
            Some(rect) => rect.size.h += 1,
            None => rects.push(Rectangle::new((x0, y).into(), (x1 - x0 + 1, 1).into())),
        }
    }
    rects
}

/// The classic 4-edge border for a square (non-rounded) frame.
fn plain_border_ring_rects(
    window_rect: Rectangle<i32, Logical>,
    bw: i32,
) -> Vec<Rectangle<i32, Logical>> {
    let (x, y, w, h) = (
        window_rect.loc.x,
        window_rect.loc.y,
        window_rect.size.w,
        window_rect.size.h,
    );
    vec![
        Rectangle::new((x - bw, y - bw).into(), (w + 2 * bw, bw).into()),
        Rectangle::new((x - bw, y + h).into(), (w + 2 * bw, bw).into()),
        Rectangle::new((x - bw, y).into(), (bw, h).into()),
        Rectangle::new((x + w, y).into(), (bw, h).into()),
    ]
}

/// Inclusive horizontal interval(s) covered by a rounded rect (radius `r`) at
/// row `y`. Returns at most one interval.
fn rounded_rect_row_spans(rect: Rectangle<i32, Logical>, r: i32, y: i32) -> Vec<(i32, i32)> {
    let l = rect.loc.x;
    let t = rect.loc.y;
    let right = rect.loc.x + rect.size.w - 1;
    let b = rect.loc.y + rect.size.h - 1;

    if y < t || y > b {
        return Vec::new();
    }
    if r <= 0 {
        return vec![(l, right)];
    }

    let dy = if y < t + r {
        y - t
    } else if y > b - r {
        b - y
    } else {
        return vec![(l, right)];
    };
    if dy < 0 {
        return vec![(l, right)];
    }

    let h = (dy as f64) - (r as f64);
    let dist = ((r as f64) * (r as f64) - h * h).sqrt();
    let cut = dist.floor() as i32;

    let x0 = (l + r - cut).max(l);
    let x1 = (right - r + cut).min(right);
    if x0 > x1 {
        return Vec::new();
    }
    vec![(x0, x1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::Point;

    #[test]
    fn effect_names_gate_animation() {
        assert!(is_animated_border_effect("breathing"));
        assert!(is_animated_border_effect("rainbow"));
        assert!(is_animated_border_effect("gradient"));
        assert!(is_animated_border_effect("flow"));
        assert!(is_animated_border_effect("glow"));
        assert!(is_animated_border_effect("pulse"));
        assert!(is_animated_border_effect("ember"));
        assert!(is_animated_border_effect("rainbow_solid"));
        assert!(is_animated_border_effect("chase"));
        assert!(is_animated_border_effect("bg"));
        assert!(is_animated_border_effect("bg-2"));
        assert!(is_animated_border_effect("bg-8"));
        assert!(!is_animated_border_effect("none"));
        assert!(!is_animated_border_effect("bogus"));
        assert!(!is_animated_border_effect("bg-foo"));
    }

    #[test]
    fn effect_colors_stay_sane() {
        let base = [1.0, 0.0, 0.5, 1.0];
        let grad = [0.0, 0.85, 1.0, 1.0];
        assert_eq!(border_effect_color(base, "none", 3.0, 0.25, grad), base);
        assert_eq!(border_effect_color(base, "bogus", 3.0, 0.25, grad), base);
        for effect in [
            "breathing",
            "rainbow",
            "rainbow_solid",
            "gradient",
            "flow",
            "glow",
            "pulse",
            "ember",
            "chase",
        ] {
            for (t, angle) in [(0.0, 0.0), (1.7, 0.3), (9.25, 0.99)] {
                let c = border_effect_color(base, effect, t, angle, grad);
                assert_eq!(c[3], 1.0, "{effect} must preserve alpha");
                assert!(
                    c[0].is_finite() && c[1].is_finite() && c[2].is_finite(),
                    "{effect} produced a non-finite channel: {c:?}"
                );
                assert!(
                    (0.0..=1.0).contains(&c[0])
                        && (0.0..=1.0).contains(&c[1])
                        && (0.0..=1.0).contains(&c[2]),
                    "{effect} escaped 0..1: {c:?}"
                );
            }
        }
        let dim = border_effect_color(base, "breathing", 0.0, 0.0, grad);
        assert!(dim[0] < base[0] && dim[0] > 0.0);
        assert!((0.0..1.0).contains(&ring_angle((10.0, 0.0), (0.0, 0.0))));
        assert!((0.0..1.0).contains(&ring_angle((0.0, -5.0), (0.0, 0.0))));
    }

    #[test]
    fn bg_count_parses_and_clamps() {
        assert_eq!(bg_effect_count("bg"), Some(4));
        assert_eq!(bg_effect_count("bg-2"), Some(2));
        assert_eq!(bg_effect_count("bg-5"), Some(5));
        assert_eq!(bg_effect_count("bg-1"), Some(2));
        assert_eq!(bg_effect_count("bg-99"), Some(8));
        assert_eq!(bg_effect_count("bg-"), None);
        assert_eq!(bg_effect_count("bg-foo"), None);
        assert_eq!(bg_effect_count("none"), None);
    }

    #[test]
    fn palette_extraction_finds_distinct_colors() {
        let mut img = image::RgbaImage::new(100, 40);
        for y in 0..40 {
            for x in 0..100 {
                let px = if x < 45 {
                    image::Rgba([255, 0, 0, 255])
                } else if x < 55 {
                    image::Rgba([0, 255, 0, 255])
                } else {
                    image::Rgba([0, 0, 255, 255])
                };
                img.put_pixel(x, y, px);
            }
        }
        let pal = wallpaper_palette(&img, 8);
        assert!(pal.len() >= 2, "expected red+blue, got {pal:?}");
        let has_red = pal.iter().any(|c| c[0] > 0.9 && c[1] < 0.2 && c[2] < 0.2);
        let has_blue = pal.iter().any(|c| c[2] > 0.9 && c[0] < 0.2 && c[1] < 0.2);
        assert!(has_red && has_blue, "lost a dominant color: {pal:?}");
        let flat = image::RgbaImage::from_pixel(60, 60, image::Rgba([40, 40, 42, 255]));
        let pal = wallpaper_palette(&flat, 8);
        assert_eq!(pal.len(), 1, "flat image should give one color: {pal:?}");
    }

    #[test]
    fn bg_effect_runs_off_the_cached_palette() {
        let mut img = image::RgbaImage::new(80, 20);
        for y in 0..20 {
            for x in 0..80 {
                img.put_pixel(
                    x,
                    y,
                    if x < 40 {
                        image::Rgba([255, 0, 0, 255])
                    } else {
                        image::Rgba([0, 0, 255, 255])
                    },
                );
            }
        }
        note_wallpaper_colors(&img);
        let base = [0.1, 0.9, 0.1, 1.0];
        let grad = [0.0, 0.0, 0.0, 1.0];
        let c = border_effect_color(base, "bg-2", 0.0, 0.25, grad);
        assert_eq!(c[3], 1.0);
        assert!(
            c[0] > 0.3 && c[2] > 0.3 && c[1] < 0.3,
            "expected a red/blue blend, got {c:?}"
        );
        let c = border_effect_color(base, "bg", 1.3, 0.6, grad);
        assert!(c[0].is_finite() && c[1].is_finite() && c[2].is_finite());
    }

    #[test]
    fn ring_rect_count_is_sane() {
        for (name, w, h, max) in [("medium", 700, 420, 80), ("large", 2560, 1600, 200)] {
            let rect = Rectangle::from_loc_and_size(Point::from((0, 0)), (w, h));
            let rects = border_ring_rects(rect, 2, 12);
            eprintln!("{name}: {} rects", rects.len());
            assert!(
                rects.len() <= max,
                "{name}: expected <= {max} rects, got {}",
                rects.len()
            );
        }
    }
}
