//! The now-playing waveform bars (closed notch, Home and Music pages).

use super::*;

/// Bars in the closed now-playing waveform.
pub(crate) const WAVE_BARS: usize = 6;

/// Height of waveform bar `i` (0–1 of its full height) at `time` seconds: taller in the
/// middle, each bar moving at its own pace; low and still when paused.
pub(crate) fn wave_level(i: usize, time: f32, playing: bool) -> f32 {
    const ENVELOPE: [f32; WAVE_BARS] = [0.45, 0.75, 1.0, 0.85, 0.6, 0.4];
    const SPEEDS: [f32; WAVE_BARS] = [6.1, 8.3, 7.2, 9.4, 6.8, 7.9];
    const PHASES: [f32; WAVE_BARS] = [0.0, 2.1, 0.9, 3.3, 1.6, 2.7];
    if !playing {
        return 0.15;
    }
    ENVELOPE[i] * (0.35 + 0.65 * (0.5 + 0.5 * (time * SPEEDS[i] + PHASES[i]).sin()).powf(1.3))
}

impl App {
    /// The waveform inside the open card: grows from the closed notch's size to full size
    /// as the notch opens. `right`/`cy` in pixels.
    pub(crate) unsafe fn draw_open_waveform(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, right: f32, cy: f32, alpha: f32) {
        let s = self.dpi_scale;
        let o = self.openness().clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| (a + (b - a) * o) * s;
        self.draw_waveform(t, brush, right, cy, lerp(2.5, 3.0), lerp(2.0, 2.5), lerp(16.0, 20.0), alpha);
    }

    /// The waveform (as the iPhone's): WAVE_BARS bars in the album's colour, centred on
    /// `cy`, right edge at `right`. Moving while playing, low and still while paused, hidden
    /// when nothing is loaded.
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn draw_waveform(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, right: f32, cy: f32, bar_w: f32, gap: f32, max_h: f32, alpha: f32) {
        let Some(session) = self.current().filter(|s| s.status != Status::Stopped) else { return };
        let accent = session.art.as_ref().map(|a| a.accent).unwrap_or([0.95, 0.65, 0.25]);
        self.brush.SetColor(&D2D1_COLOR_F { r: accent[0], g: accent[1], b: accent[2], a: 1.0 });
        self.brush.SetOpacity(alpha);
        let time = self.started.elapsed().as_secs_f32();
        let playing = session.status == Status::Playing;
        for i in 0..WAVE_BARS {
            let bar_h = (max_h * wave_level(i, time, playing)).max(bar_w);
            let x = right - (WAVE_BARS - i) as f32 * (bar_w + gap) + gap;
            let r = D2D1_ROUNDED_RECT { rect: rect(x, cy - bar_h / 2.0, x + bar_w, cy + bar_h / 2.0), radiusX: bar_w / 2.0, radiusY: bar_w / 2.0 };
            t.FillRoundedRectangle(&r, brush);
        }
    }
}

#[cfg(test)]
#[path = "tests/waveform.rs"]
mod tests;
