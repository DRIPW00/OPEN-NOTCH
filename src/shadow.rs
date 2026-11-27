//! The notch's soft shadow: a per-pixel CPU shader painted into the shadow window.

use super::*;

/// The notch's outline in pixels: a box hanging from the top edge, centred at `cx`,
/// `w` wide and `h` tall, with round bottom corners of radius `r`. Its top corners flare
/// out into the screen edge over `shoulder` pixels.
pub(crate) struct Outline {
    cx: f32,
    w: f32,
    h: f32,
    r: f32,
    shoulder: f32,
}

impl Outline {
    /// Signed distance from (x, y) to the outline: > 0 outside, < 0 inside, so the shadow
    /// hugs the shape the same distance all the way round: round bottom corners, and the
    /// curve where each top corner flares out into the screen edge.
    fn distance(&self, x: f32, y: f32) -> f32 {
        let ax = (x - self.cx).abs(); // the shape is mirror-symmetric
        let half = self.w / 2.0;
        // The body: a box hanging from the top, round bottom corners.
        let qx = ax - (half - self.r);
        let qy = y - (self.h - self.r);
        let body = length(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - self.r;
        if self.shoulder <= 0.0 {
            return body;
        }
        // The flare: a concave quarter round of radius `shoulder`, centred at
        // (half + shoulder, shoulder), from the side up to its tip at the screen edge.
        let sh = self.shoulder;
        let (px, py) = (ax - half - sh, y - sh);
        if py > 0.0 {
            body // below the flare: the straight side
        } else if px <= 0.0 {
            body.min(sh - length(px, py)) // facing the curve: straight out from its centre
        } else {
            body.min(length(px, py + sh)) // beyond it: the tip is nearest
        }
    }
}

/// The complementary error function, to within 1.5e-7 (Abramowitz & Stegun 7.1.26).
pub(crate) fn erfc(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = t * (0.254_829_6 + t * (-0.284_496_7 + t * (1.421_413_7 + t * (-1.453_152_1 + t * 1.061_405_4))));
    let e = poly * (-x * x).exp();
    if x >= 0.0 { e } else { 2.0 - e }
}

pub(crate) fn length(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

/// How the shadow looks, in pixels: blur width, darkness at the edge, shift down.
pub(crate) struct ShadowLook {
    sigma: f32,
    peak: f32,
    drop: f32,
}

impl ShadowLook {
    /// The shader: opacity at `d` pixels outside the (shifted) outline. The notch's shape
    /// blurred by a Gaussian of width `sigma`, as a real soft shadow (and CSS box-shadow):
    /// half strength right at the edge, so the notch's own edge stays crisp, smoothly
    /// darker underneath and lighter outside, gone (under 1/255) by about 3 sigma.
    fn alpha(&self, d: f32) -> f32 {
        self.peak * 0.5 * erfc(d / (self.sigma * std::f32::consts::SQRT_2))
    }

    /// How far from the edge the shadow can show.
    fn reach(&self) -> f32 {
        3.5 * self.sigma + self.drop
    }
}

/// Paint the shadow into premultiplied BGRA pixels (black, so only alpha is set). Runs the
/// shader for every pixel in the thin band around the outline; the notch window covers
/// the rest. Each pixel gets a fixed random offset under one 8-bit step before rounding
/// (dithering), so the fade has no visible bands.
pub(crate) fn paint_shadow(pixels: &mut [u32], width: usize, height: usize, notch: &Outline, look: &ShadowLook) {
    pixels.fill(0);
    if look.peak <= 0.0 || notch.w <= 0.0 {
        return;
    }
    const INSIDE: f32 = 2.0; // a little under the notch's anti-aliased rim, so no gap shows
    let reach = look.reach();
    let side = notch.w / 2.0 + notch.shoulder + reach; // the flare tips stick out past the sides
    let x0 = (notch.cx - side).floor().max(0.0) as usize;
    let x1 = ((notch.cx + side).ceil() as usize).min(width);
    let y1 = ((notch.h + reach).ceil() as usize).min(height);
    // Columns well inside the notch, skipped on the rows above its bottom corners.
    let skip0 = (notch.cx - notch.w / 2.0 + INSIDE).ceil().max(0.0) as usize;
    let skip1 = (notch.cx + notch.w / 2.0 - INSIDE).floor().max(0.0) as usize;
    for y in 0..y1 {
        // Shadow all the way up to the screen edge, so the flared top corners have it too.
        // The downward shift eases to nothing at the top: shifted there, the shadow pulled
        // away from the flare's curve and pooled past its tip.
        let ease = ((y as f32 + 0.5) / (notch.shoulder + 2.0 * look.sigma)).clamp(0.0, 1.0);
        let drop = look.drop * ease * ease * (3.0 - 2.0 * ease);
        let fy = y as f32 + 0.5 - drop;
        let straight = (y as f32 + 0.5) < notch.h - notch.r; // above the real bottom corners
        let mut x = x0;
        while x < x1 {
            if straight && x >= skip0 && x < skip1 {
                x = skip1;
                continue;
            }
            // d is measured from the outline shifted down by `drop`: a pixel just inside the
            // real bottom edge is `drop` further in, so allow for it (or the rim shows through).
            let d = notch.distance(x as f32 + 0.5, fy);
            if d > -(INSIDE + drop) {
                let a = look.alpha(d) * 255.0 + dither(x, y);
                pixels[y * width + x] = (a.clamp(0.0, 255.0) as u32) << 24;
            }
            x += 1;
        }
    }
}

/// A fixed pseudo-random value in [0, 1) for each pixel (a hash, so it doesn't flicker).
pub(crate) fn dither(x: usize, y: usize) -> f32 {
    let mut n = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    n ^= n >> 15;
    n = n.wrapping_mul(0x2C1B_3C6D);
    n ^= n >> 12;
    (n & 0xFFFF) as f32 / 65536.0
}

impl App {
    /// The soft shadow under the notch, drawn into the click-through shadow window. Only
    /// redrawn when the notch's size changes (not for every waveform frame).
    pub(crate) unsafe fn render_shadow(&mut self) {
        if self.shadow_hwnd.is_invalid() {
            return;
        }
        let key = ((self.width.value * 2.0).round() as i32, (self.height.value * 2.0).round() as i32);
        if key == self.shadow_key {
            return;
        }
        self.shadow_key = key;
        if self.shadow_bits.is_null() {
            return;
        }
        let s = self.dpi_scale;
        let (width, height) = (self.px_w as usize, self.px_h as usize);
        let _ = GdiFlush(); // GDI may still be using the bitmap
        let pixels = std::slice::from_raw_parts_mut(self.shadow_bits, width * height);
        // No shadow on the thin bar; full shadow once the notch is its normal size. A little
        // deeper and softer when open.
        let strength = ((self.height.value - BAR.1) / (CLOSED.1 - BAR.1)).clamp(0.0, 1.0);
        let open = self.openness().clamp(0.0, 1.0);
        let notch = Outline {
            cx: width as f32 / 2.0,
            w: self.width.value * s,
            h: self.height.value * s,
            r: self.corner_radius(),
            shoulder: (SHOULDER * s).min(self.height.value * s / 2.0),
        };
        let look = ShadowLook {
            sigma: (3.0 + 1.5 * open) * s,
            peak: (0.62 + 0.08 * open) * strength,
            drop: (1.0 + 1.5 * open) * s,
        };
        paint_shadow(pixels, width, height, &notch, &look);
        let size = SIZE { cx: self.px_w, cy: self.px_h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let _ = UpdateLayeredWindow(self.shadow_hwnd, None, None, Some(&size), Some(self.shadow_dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_notch() -> Outline {
        Outline { cx: 400.0, w: 300.0, h: 200.0, r: 30.0, shoulder: 10.0 }
    }

    // Shadow shader: the distance field matches the notch's shape.
    #[test]
    fn outline_distance_is_exact() {
        let n = sample_notch();
        assert!(n.distance(400.0, 200.0).abs() < 1e-4); // bottom edge
        assert!((n.distance(400.0, 210.0) - 10.0).abs() < 1e-4); // 10 below
        assert!((n.distance(560.0, 50.0) - 10.0).abs() < 1e-4); // 10 right of the side
        assert!((n.distance(400.0, 150.0) + 50.0).abs() < 1e-4); // inside
        // Corner: 10 px out along the diagonal from the corner's centre.
        let (ccx, ccy) = (550.0 - 30.0, 200.0 - 30.0);
        let k = (30.0 + 10.0) / 2f32.sqrt();
        assert!((n.distance(ccx + k, ccy + k) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn shadow_is_thin_smooth_and_hugs_the_edge() {
        for sigma in [3.0, 4.5] {
            let look = ShadowLook { sigma, peak: 0.5, drop: 1.0 };
            let mut last = look.alpha(0.0);
            for tenth in 1..300 {
                let now = look.alpha(tenth as f32 / 10.0);
                assert!(now <= last, "only gets lighter going out");
                assert!(last - now < 0.02, "no hard step ({last} -> {now})");
                last = now;
            }
            // Gone (under one 8-bit step) within ~3.5 sigma of the edge: a few pixels.
            assert!(look.alpha(look.reach() - look.drop) * 255.0 < 0.5);
            assert!(look.reach() < 18.0, "stays close to the edge ({})", look.reach());
        }
    }

    #[test]
    fn painted_shadow_has_no_banding() {
        let n = Outline { cx: 400.0, w: 300.0, h: 600.0, r: 30.0, shoulder: 10.0 };
        let look = ShadowLook { sigma: 3.0, peak: 0.45, drop: 0.0 };
        let (w, h) = (800usize, 700usize);
        let mut px = vec![0u32; w * h];
        paint_shadow(&mut px, w, h, &n, &look);
        // Down the straight right side, each column's average matches the smooth curve to
        // well within one 8-bit step: dithered, not stair-stepped.
        let right = (n.cx + n.w / 2.0) as usize;
        for x in right..right + 10 {
            let mean: f32 = (100..500).map(|y| (px[y * w + x] >> 24) as f32).sum::<f32>() / 400.0;
            let want = look.alpha(x as f32 + 0.5 - (n.cx + n.w / 2.0)) * 255.0;
            assert!((mean - want).abs() < 0.3, "column {x}: {mean} vs {want}");
        }
        // Nothing beyond the reach; nothing deep inside; colour stays black (premultiplied).
        assert!(px.iter().all(|p| p & 0x00FF_FFFF == 0));
        assert_eq!(px[300 * w + right + 20] >> 24, 0);
        assert_eq!(px[300 * w + 400] >> 24, 0);
    }

    #[test]
    fn painted_shadow_fits_the_window() {
        // A notch wider than the window, and a zero-strength shadow, are both safe.
        let mut px = vec![7u32; 50 * 20];
        paint_shadow(&mut px, 50, 20, &Outline { cx: 25.0, w: 80.0, h: 30.0, r: 10.0, shoulder: 8.0 }, &ShadowLook { sigma: 3.0, peak: 0.4, drop: 1.0 });
        paint_shadow(&mut px, 50, 20, &sample_notch(), &ShadowLook { sigma: 3.0, peak: 0.0, drop: 1.0 });
        assert!(px.iter().all(|p| *p == 0));
    }

    #[test]
    fn flare_distance_is_exact() {
        // sample_notch: half width 150 (right side at x 550), flare radius 10.
        let n = sample_notch();
        // Facing the flare's curve: straight out from its centre (560, 10).
        let d = n.distance(553.0, 3.0);
        assert!((d - (10.0 - (49.0f32 * 2.0).sqrt())).abs() < 1e-4, "{d}");
        assert!(n.distance(551.0, 1.0) < 0.0, "inside the flare");
        // Beyond the flare: its tip at the screen edge (560, 0) is nearest.
        assert!((n.distance(565.0, 0.0) - 5.0).abs() < 1e-4);
        assert!((n.distance(563.0, 4.0) - 5.0).abs() < 1e-4);
        // Below it: the straight side again.
        assert!((n.distance(556.0, 40.0) - 6.0).abs() < 1e-4);
    }

    #[test]
    fn shadow_wraps_the_flare_smoothly() {
        // The shadow must run round the flared top corners right up to the screen edge (the
        // flares were bare before), with no jumps down the side and no crease out to the side.
        let n = sample_notch();
        let look = ShadowLook { sigma: 2.5, peak: 0.45, drop: 1.0 };
        let (w, h) = (800usize, 300usize);
        let mut px = vec![0u32; w * h];
        paint_shadow(&mut px, w, h, &n, &look);
        let a = |x: usize, y: usize| (px[y * w + x] >> 24) as i32;
        assert!(a(561, 0) > 0, "shadow at the flare's tip, on the screen edge row");
        // Just outside the side, top to bottom: no jumps.
        let side = 550; // pixel centre half a pixel outside the right side
        let column: Vec<i32> = (3..80).map(|y| a(side, y)).collect(); // rows 0-2: under the flare
        assert!(column.windows(2).all(|p| (p[1] - p[0]).abs() <= 12), "{column:?}");
        let want = (look.alpha(0.5) * 255.0) as i32;
        assert!((column[57] - want).abs() <= 1, "full shadow down the side: {} vs {want}", column[57]);
        // Out to the side at the flare's height: fades away, no crease.
        let row: Vec<i32> = (550..590).map(|x| a(x, 10)).collect();
        assert!(row.windows(2).all(|p| p[1] <= p[0] + 2), "only fades going out: {row:?}");
    }

    #[test]
    fn notch_edge_stays_crisp() {
        // On-screen review: the darkest shadow sat right against the edge and blurred the
        // rounded corners. Now the shadow is the notch's shape blurred (as CSS box-shadow):
        // half strength at the edge, darker underneath, lighter outside.
        let look = ShadowLook { sigma: 2.5, peak: 0.6, drop: 0.0 };
        assert!((look.alpha(0.0) - 0.3).abs() < 1e-3);
        assert!(look.alpha(-10.0) > 0.59 && look.alpha(10.0) < 0.001);
        assert!((look.alpha(1.0) + look.alpha(-1.0) - 0.6).abs() < 1e-3, "symmetric about the edge");
    }

    #[test]
    fn no_light_gap_under_the_bottom_edge() {
        // Found in the renders: with the shadow shifted down by more than 2 px, the pixels on
        // the notch's own bottom edge were skipped and the background showed through its
        // anti-aliased rim as a bright line. Every rim pixel must be shaded.
        let n = sample_notch();
        for drop in [0.0, 1.0, 2.75, 4.0] {
            let look = ShadowLook { sigma: 2.0, peak: 0.6, drop };
            let (w, h) = (800usize, 300usize);
            let mut px = vec![0u32; w * h];
            paint_shadow(&mut px, w, h, &n, &look);
            for y in [198usize, 199] {
                let a = px[y * w + 400] >> 24;
                assert!(a as f32 >= look.alpha(0.0) * 255.0 - 1.0, "drop {drop}: rim row {y} = {a}");
            }
        }
    }

    #[test]
    fn every_edge_has_shadow() {
        // On-screen review: too thin, and the flared top corners were bare. Every pixel just
        // outside the outline (sides, round bottom corners, flares up to the screen edge)
        // must get a clear share of the shadow.
        let n = sample_notch();
        let look = ShadowLook { sigma: 3.0, peak: 0.62, drop: 1.0 };
        let (w, h) = (800usize, 300usize);
        let mut px = vec![0u32; w * h];
        paint_shadow(&mut px, w, h, &n, &look);
        let mut checked = 0;
        for y in 0..h {
            for x in 0..w {
                let d = n.distance(x as f32 + 0.5, y as f32 + 0.5);
                if d > 0.25 && d <= 1.25 {
                    let a = (px[y * w + x] >> 24) as f32;
                    assert!(a >= 0.25 * look.peak * 255.0, "bare edge at ({x}, {y}): {a}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 600, "walked the whole outline ({checked})");
    }

    #[test]
    fn flare_shadow_is_even_up_to_the_screen_edge() {
        // On-screen review: shifted down, the shadow pulled away from the flare's curve near
        // the top and pooled into a lump past its tip. Just outside the curve it must match
        // the straight side's shade all the way up.
        let n = sample_notch(); // right side at x 550, flare centre (560, 10), radius 10
        let look = ShadowLook { sigma: 3.0, peak: 0.62, drop: 2.5 };
        let (w, h) = (800usize, 300usize);
        let mut px = vec![0u32; w * h];
        paint_shadow(&mut px, w, h, &n, &look);
        let a = |x: f32, y: f32| (px[y as usize * w + x as usize] >> 24) as f32;
        let side = a(551.0, 60.0);
        for deg in (180..=265).step_by(5) {
            let t = (deg as f32).to_radians();
            let (x, y) = (560.0 + 8.8 * t.cos(), 10.0 + 8.8 * t.sin());
            let v = a(x, y);
            // A concave curve catches a little more shadow than a straight side, but no lump.
            assert!(v >= side * 0.8 && v <= side * 1.6, "{deg}°: {v} vs side {side}");
        }
    }

    #[test]
    fn erfc_is_accurate() {
        for (x, want) in [(0.0, 1.0), (0.5, 0.479_500_1), (1.0, 0.157_299_2), (2.0, 0.004_677_7), (-1.0, 1.842_700_8)] {
            assert!((erfc(x) - want).abs() < 1e-5, "erfc({x}) = {}", erfc(x));
        }
    }
}
