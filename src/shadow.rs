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
#[path = "tests/shadow.rs"]
mod tests;
