//! The closed notch and the Home view: clock, now playing, privacy dots and bubble, and the volume/brightness pill.

use super::*;

impl App {
    /// Battery ring colour: green charging, red low, yellow battery saver, else white.
    pub(crate) fn battery_colour(b: &sysinfo::Battery) -> u32 {
        if b.charging {
            0x30D158
        } else if b.percent <= 20 {
            0xFF453A
        } else if b.saver {
            0xFFD60A
        } else {
            0xFFFFFF
        }
    }

    /// Camera/mic indicators to show, camera first: (colour, icon, flag).
    pub(crate) fn privacy_colours(&self) -> Vec<(u32, Icon, u8)> {
        let mut v = Vec::new();
        if self.privacy & privacy::CAMERA != 0 {
            v.push((CAMERA_GREEN, Icon::Camera, privacy::CAMERA));
        }
        if self.privacy & privacy::MIC != 0 {
            v.push((MIC_ORANGE, Icon::Mic, privacy::MIC));
        }
        v
    }

    /// The small bubble under a hovered camera/mic indicator: "Microphone · Discord, …".
    pub(crate) unsafe fn draw_privacy_bubble(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, flag: u8, anchor: &D2D_RECT_F, pop: f32, fade: f32) {
        let s = self.dpi_scale;
        let (title, colour) = if flag == privacy::CAMERA { ("Camera", CAMERA_GREEN) } else { ("Microphone", MIC_ORANGE) };
        let apps = self.privacy_info.as_ref().map(|p| p.users(flag)).unwrap_or_default();
        let names = if apps.is_empty() { "In use".to_string() } else { apps.join(", ") };
        let title_text = format!("{title}  \u{00B7}  ");

        let fonts = &self.gfx.fonts;
        let title_w = self.gfx.measure(&title_text, &fonts.bubble_title);
        let names_w = self.gfx.measure(&names, &fonts.bubble_text);
        let (pad_x, height, arrow) = (11.0 * s, 28.0 * s, 6.0 * s);
        let max_w = self.px_w as f32 - 16.0 * s;
        let width = (pad_x * 2.0 + title_w + names_w + 1.0).min(max_w);

        // Below the card, centred under the indicator, kept inside the window.
        let anchor_x = (anchor.left + anchor.right) / 2.0;
        let top = self.height.value * s + 8.0 * s + arrow;
        let left = (anchor_x - width / 2.0).clamp(8.0 * s, self.px_w as f32 - 8.0 * s - width);
        let body = D2D1_ROUNDED_RECT { rect: rect(left, top, left + width, top + height), radiusX: 9.0 * s, radiusY: 9.0 * s };

        // Pop: grow from 85% and slide down from under the notch, fading in.
        let k = 0.85 + 0.15 * pop;
        let (ax, ay) = (anchor_x, top - arrow);
        let dy = (1.0 - pop) * -10.0 * s;
        t.SetTransform(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: ax - k * ax, M32: ay - k * ay + dy });
        let alpha = pop.clamp(0.0, 1.0) * fade;
        self.brush.SetOpacity(alpha);
        self.brush.SetColor(&rgb(0x1C1C1E, 1.0));
        t.FillRoundedRectangle(&body, brush);
        // Little arrow pointing up at the indicator.
        if let Ok(geometry) = self.factory.CreatePathGeometry() {
            if let Ok(sink) = geometry.Open() {
                sink.BeginFigure(Vector2 { X: anchor_x - arrow, Y: top + 0.5 }, D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLine(Vector2 { X: anchor_x, Y: top - arrow });
                sink.AddLine(Vector2 { X: anchor_x + arrow, Y: top + 0.5 });
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                if sink.Close().is_ok() {
                    t.FillGeometry(&geometry, brush, None);
                }
            }
        }
        self.brush.SetColor(&rgb(0x3A3A3C, 1.0));
        t.DrawRoundedRectangle(&body, brush, 1.0 * s, None);

        self.brush.SetColor(&rgb(colour, 1.0));
        self.gfx.text(t, &title_text, &fonts.bubble_title, rect(left + pad_x, top, left + pad_x + title_w + 1.0, top + height), brush);
        self.brush.SetColor(&rgb(0xFFFFFF, 1.0));
        self.gfx.text(t, &names, &fonts.bubble_text, rect(left + pad_x + title_w, top, left + width - pad_x, top + height), brush);
        t.SetTransform(&Matrix3x2::identity());
    }

    /// Small coloured dots for camera/mic, ending at `right_x` (right to left).
    pub(crate) unsafe fn draw_privacy_dots(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, right_x: f32, cy: f32, alpha: f32) {
        let s = self.dpi_scale;
        let dots = self.privacy_colours();
        for (i, (colour, _, _)) in dots.iter().rev().enumerate() {
            self.brush.SetColor(&rgb(*colour, 1.0));
            self.brush.SetOpacity(alpha);
            let x = right_x - 3.5 * s - i as f32 * 11.0 * s;
            t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x, Y: cy }, radiusX: 3.5 * s, radiusY: 3.5 * s }, brush);
        }
    }

    /// Closed, nothing playing (design 6A): "9:41 THU 26" on the left; camera/mic dots and
    /// the battery ring on the right.
    /// Closed, no slider showing: the clock, or (while music plays) now playing, cross-faded.
    pub(crate) unsafe fn draw_idle_closed(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
        let mix = self.music_mix.value.clamp(0.0, 1.0);
        if mix < 0.999 {
            self.draw_clock_closed(t, brush, alpha * (1.0 - mix));
        }
        if mix > 0.001 {
            self.draw_now_playing_closed(t, brush, alpha * mix);
        }
    }

    /// Closed while music plays, as the iPhone: album art on the left, a waveform in the
    /// album's colour on the right (camera/mic dots stay, just left of it).
    pub(crate) unsafe fn draw_now_playing_closed(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
        let Some(session) = self.current() else { return };
        let s = self.dpi_scale;
        let w = self.width.value * s;
        let cx = self.px_w as f32 / 2.0;
        let (left, right) = (cx - w / 2.0, cx + w / 2.0);
        let h = 32.0 * s;
        let size = 22.0 * s;
        self.draw_art(t, brush, session, left + 12.0 * s, (h - size) / 2.0, size, 6.0 * s, alpha, false);
        let wave_right = right - 14.0 * s;
        self.draw_waveform(t, brush, wave_right, h / 2.0, 2.5 * s, 2.0 * s, 16.0 * s, alpha);
        let wave_w = WAVE_BARS as f32 * 4.5 * s;
        let dots_right = wave_right - wave_w - 8.0 * s;
        self.draw_privacy_dots(t, brush, dots_right, h / 2.0, alpha);
        // The song between the art and the waveform: title, then the artist if there's room.
        let dots_w = self.privacy_colours().len() as f32 * 11.0 * s;
        let (text_left, text_right) = (left + 42.0 * s, dots_right - dots_w - 6.0 * s);
        if text_right - text_left < 20.0 * s {
            return; // the notch is mid-animation and too narrow for text
        }
        let fonts = &self.gfx.fonts;
        self.brush.SetColor(&rgb(0xFFFFFF, 1.0));
        self.brush.SetOpacity(alpha);
        self.gfx.text(t, &session.title, &fonts.closed_song, rect(text_left, 0.0, text_right, h), brush);
        let artist_left = text_left + self.gfx.measure(&session.title, &fonts.closed_song) + 6.0 * s;
        if !session.artist.is_empty() && text_right - artist_left > 30.0 * s {
            self.brush.SetColor(&rgb(0x8E8E93, 1.0));
            self.gfx.text(t, &session.artist, &fonts.closed_song, rect(artist_left, 0.0, text_right, h), brush);
        }
    }

    /// The new-song pop-up: [art] title / artist … time left, waveform.
    pub(crate) unsafe fn draw_track_hud(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
        let Some(session) = self.current() else { return };
        let s = self.dpi_scale;
        let w = self.width.value * s;
        let h = self.height.value * s;
        let cx = self.px_w as f32 / 2.0;
        let (left, right) = (cx - w / 2.0, cx + w / 2.0);
        let size = (h - 16.0 * s).max(0.0);
        self.draw_art(t, brush, session, left + 10.0 * s, 8.0 * s, size, 8.0 * s, alpha, true);
        let text_left = left + 10.0 * s + size + 10.0 * s;
        let text_right = right - 62.0 * s;
        let fonts = &self.gfx.fonts;
        self.brush.SetOpacity(alpha);
        self.brush.SetColor(&rgb(0xFFFFFF, 1.0));
        self.gfx.text(t, &session.title, &fonts.title, rect(text_left, 8.0 * s, text_right, h / 2.0 + 2.0 * s), brush);
        self.brush.SetColor(&rgb(0xA1A1AA, 1.0));
        self.gfx.text(t, &session.artist, &fonts.artist, rect(text_left, h / 2.0, text_right, h - 8.0 * s), brush);
        if session.duration > 0.0 {
            self.gfx.text(t, &time_left(session.position_now(), session.duration), &fonts.time_right, rect(right - 60.0 * s, 8.0 * s, right - 14.0 * s, h / 2.0), brush);
        }
        self.draw_waveform(t, brush, right - 14.0 * s, h * 0.7, 2.5 * s, 2.0 * s, 14.0 * s, alpha);
    }

    /// Closed, nothing loaded: [time] [day] [shelf files] … [dots][battery].
    pub(crate) unsafe fn draw_clock_closed(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
        let s = self.dpi_scale;
        let w = self.width.value * s;
        let cx = self.px_w as f32 / 2.0;
        let (left, right) = (cx - w / 2.0, cx + w / 2.0);
        let h = 32.0 * s;

        self.brush.SetColor(&rgb(0xFFFFFF, 1.0));
        self.brush.SetOpacity(alpha);
        let x = left + 16.0 * s;
        self.gfx.text(t, &self.clock.short, &self.gfx.fonts.clock, rect(x, 0.0, cx + 40.0 * s, h), brush);
        let time_w = self.gfx.measure(&self.clock.short, &self.gfx.fonts.clock);
        self.brush.SetColor(&rgb(DAY_RED, 1.0));
        let day = format!("{} {}", self.clock.weekday_short, self.clock.day);
        self.gfx.text(t, &day, &self.gfx.fonts.clock_day, rect(x + time_w + 8.0 * s, 0.0, cx + 60.0 * s, h), brush);

        // Files on the shelf: up to three overlapping mini thumbnails after the day.
        if !self.shelf.is_empty() {
            let day_w = self.gfx.measure(&day, &self.gfx.fonts.clock_day);
            let mut tx = x + time_w + 8.0 * s + day_w + 10.0 * s;
            let size = 18.0 * s;
            for path in self.shelf.iter().rev().take(3) {
                let border = 2.0 * s;
                self.brush.SetColor(&rgb(0x000000, 1.0));
                self.brush.SetOpacity(alpha);
                let outer = rect(tx - border, h / 2.0 - size / 2.0 - border, tx + size + border, h / 2.0 + size / 2.0 + border);
                t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: outer, radiusX: 6.0 * s, radiusY: 6.0 * s }, brush);
                self.draw_file_thumb(t, brush, path, tx, h / 2.0 - size / 2.0, size, 4.0 * s, alpha);
                tx += size - 6.0 * s;
            }
        }

        let mut dots_right = right - 15.0 * s;
        if let Some(b) = &self.battery {
            let (r, width) = (8.5 * s, 2.75 * s);
            let ring_cx = right - 15.0 * s - r;
            self.brush.SetColor(&rgb(0x3A3A3C, 1.0));
            t.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: ring_cx, Y: h / 2.0 }, radiusX: r, radiusY: r }, brush, width, None);
            self.brush.SetColor(&rgb(Self::battery_colour(b), 1.0));
            self.gfx.arc(t, ring_cx, h / 2.0, r, width, b.percent as f32 / 100.0, brush);
            dots_right = ring_cx - r - 10.0 * s;
        }
        self.draw_privacy_dots(t, brush, dots_right, h / 2.0, alpha);
    }

    /// Open, nothing playing (design 6A + the red bar from 6D):
    /// [red bar][calendar tile: THU / 26][9:41 AM / Thursday, November] … [camera/mic, centred]
    /// … [84% / Charging][battery ring with bolt].
    pub(crate) unsafe fn draw_idle_open(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
        let mut hits = Vec::new();
        let s = self.dpi_scale;
        let cx = self.px_w as f32 / 2.0;
        let ox = cx - OPEN_IDLE.0 / 2.0 * s;
        let at = |x: f32| ox + x * s;
        let y = |v: f32| v * s;
        let set = |hex: u32, a: f32| {
            self.brush.SetColor(&rgb(hex, 1.0));
            self.brush.SetOpacity(a * alpha);
        };
        let mid = OPEN_IDLE.1 / 2.0; // vertical centre of the card

        // Red accent bar
        set(DAY_RED, 1.0);
        let bar = D2D1_ROUNDED_RECT { rect: rect(at(20.0), y(mid - 27.0), at(23.0), y(mid + 27.0)), radiusX: y(1.5), radiusY: y(1.5) };
        t.FillRoundedRectangle(&bar, brush);

        // Calendar tile: red weekday over the day number
        let (tile_x, tile) = (37.0, 54.0);
        set(0x1C1C1E, 1.0);
        let r = D2D1_ROUNDED_RECT { rect: rect(at(tile_x), y(mid - tile / 2.0), at(tile_x + tile), y(mid + tile / 2.0)), radiusX: y(14.0), radiusY: y(14.0) };
        t.FillRoundedRectangle(&r, brush);
        set(DAY_RED, 1.0);
        self.gfx.text(t, &self.clock.weekday_short, &self.gfx.fonts.tile_day, rect(at(tile_x), y(mid - 22.0), at(tile_x + tile), y(mid - 8.0)), brush);
        set(0xFFFFFF, 1.0);
        self.gfx.text(t, &self.clock.day, &self.gfx.fonts.tile_number, rect(at(tile_x), y(mid - 9.0), at(tile_x + tile), y(mid + 21.0)), brush);

        // Time (with a small AM/PM) and "Thursday, November"
        let text_x = tile_x + tile + 14.0;
        set(0xFFFFFF, 1.0);
        self.gfx.text(t, &self.clock.short, &self.gfx.fonts.clock_big, rect(at(text_x), y(mid - 28.0), at(240.0), y(mid + 10.0)), brush);
        if !self.clock.marker.is_empty() {
            let time_w = self.gfx.measure(&self.clock.short, &self.gfx.fonts.clock_big) / s;
            set(0x98989F, 1.0);
            self.gfx.text(t, &self.clock.marker, &self.gfx.fonts.marker, rect(at(text_x + time_w + 5.0), y(mid - 12.0), at(250.0), y(mid + 6.0)), brush);
        }
        let day = format!("{},", self.clock.weekday);
        set(DAY_RED, 1.0);
        self.gfx.text(t, &day, &self.gfx.fonts.date_day, rect(at(text_x), y(mid + 9.0), at(250.0), y(mid + 27.0)), brush);
        let day_w = self.gfx.measure(&format!("{day} "), &self.gfx.fonts.date_day) / s;
        set(0x98989F, 1.0);
        self.gfx.text(t, &self.clock.month, &self.gfx.fonts.date, rect(at(text_x + day_w), y(mid + 9.0), at(250.0), y(mid + 27.0)), brush);

        // Camera / mic: always centred, with an "IN USE" caption.
        let items = self.privacy_colours();
        if !items.is_empty() {
            let (size, gap, cy) = (32.0, 8.0, mid - 8.0);
            let total = items.len() as f32 * size + (items.len() - 1) as f32 * gap;
            let mut x = OPEN_IDLE.0 / 2.0 - total / 2.0 + size / 2.0;
            for (colour, icon, flag) in items {
                let action = Action::Privacy(flag);
                let hovered = self.hover_action.as_ref() == Some(&action);
                set(colour, if hovered { 0.28 } else { 0.16 });
                t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(x), Y: y(cy) }, radiusX: y(size / 2.0), radiusY: y(size / 2.0) }, brush);
                hits.push((rect(at(x - size / 2.0), y(cy - size / 2.0), at(x + size / 2.0), y(cy + size / 2.0)), action));
                set(colour, 1.0);
                self.gfx.icon(t, icon, at(x), y(cy), y(16.0), brush);
                x += size + gap;
            }
            set(0x6E6E73, 1.0);
            let half = OPEN_IDLE.0 / 2.0;
            self.gfx.text(t, "IN USE", &self.gfx.fonts.caption, rect(at(half - 40.0), y(cy + 20.0), at(half + 40.0), y(cy + 34.0)), brush);
        }

        // Battery (design 6): "84%" and a status line beside a ring with a charging bolt.
        if let Some(b) = &self.battery {
            let colour = Self::battery_colour(b);
            let (r, width) = (24.0, 4.5);
            let ring_x = OPEN_IDLE.0 - 24.0 - r - width / 2.0;
            set(0x3A3A3C, 1.0);
            t.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(ring_x), Y: y(mid) }, radiusX: y(r), radiusY: y(r) }, brush, y(width), None);
            set(colour, 1.0);
            self.gfx.arc(t, at(ring_x), y(mid), y(r), y(width), b.percent as f32 / 100.0, brush);
            if b.charging {
                self.gfx.icon(t, Icon::Bolt, at(ring_x), y(mid), y(16.0), brush);
            }
            let text_right = ring_x - r - width / 2.0 - 12.0;
            self.gfx.text(t, &format!("{}%", b.percent), &self.gfx.fonts.battery_big, rect(at(text_right - 90.0), y(mid - 17.0), at(text_right), y(mid + 5.0)), brush);
            let status = if b.charging {
                "Charging".to_string()
            } else if b.saver {
                "Battery saver".to_string()
            } else if let Some(secs) = b.seconds_left {
                let (h, m) = (secs / 3600, (secs / 60) % 60);
                if h > 0 { format!("{h}h {m}m left") } else { format!("{m}m left") }
            } else {
                "On battery".to_string()
            };
            set(0x98989F, 1.0);
            self.gfx.text(t, &status, &self.gfx.fonts.battery_status, rect(at(text_right - 110.0), y(mid + 4.0), at(text_right), y(mid + 18.0)), brush);

            // Waveform just left of the battery text.
            let text_w = (self.gfx.measure(&format!("{}%", b.percent), &self.gfx.fonts.battery_big)
                .max(self.gfx.measure(&status, &self.gfx.fonts.battery_status)))
                / s;
            self.draw_open_waveform(t, brush, at(text_right - text_w - 16.0), y(mid), alpha);
        } else {
            // No battery (desktop PC): the waveform takes the battery's place.
            self.draw_open_waveform(t, brush, at(OPEN_IDLE.0 - 24.0), y(mid), alpha);
        }
        hits
    }

    /// Closed: always the Home view, with now playing while music is loaded.
    /// After a volume/brightness key press the clock crossfades into a slider for a moment.
    pub(crate) unsafe fn draw_closed(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
        let mix = self.hud_mix.value.clamp(0.0, 1.0);
        if mix < 0.999 {
            self.draw_idle_closed(t, brush, alpha * (1.0 - mix));
        }
        if mix > 0.001 {
            if self.hud_kind == Hud::Track {
                self.draw_track_hud(t, brush, alpha * mix);
            } else {
                self.draw_hud(t, brush, alpha * mix);
            }
        }
    }

    /// Volume (0–1) and brightness (0–1) as the sliders show them.
    pub(crate) fn level(&self, hud: Hud) -> f32 {
        match hud {
            Hud::Volume => self.sound.volume,
            Hud::Brightness => self.bright.unwrap_or(0) as f32 / 100.0,
            Hud::Track => 0.0,
        }
    }

    /// A fat rounded slider: dark track, white/yellow fill, icon (and label) on top. The
    /// icon and label are white over the track and black over the fill.
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn draw_pill(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, r: D2D_RECT_F, level: f32, fill: u32, icon: Icon, label: &str, alpha: f32) {
        let s = self.dpi_scale;
        let h = r.bottom - r.top;
        let shape = D2D1_ROUNDED_RECT { rect: r, radiusX: h / 2.0, radiusY: h / 2.0 };
        self.brush.SetOpacity(alpha);
        self.brush.SetColor(&rgb(0x2C2C2E, 1.0));
        t.FillRoundedRectangle(&shape, brush);
        let fill_x = r.left + (r.right - r.left) * level.clamp(0.0, 1.0);

        let icon_size = (h * 0.36).clamp(12.0 * s, 17.0 * s);
        let icon_x = r.left + h * 0.42 + icon_size / 2.0 - 4.0 * s;
        let text_x = icon_x + icon_size / 2.0 + 8.0 * s;
        let content = |colour: u32| {
            self.brush.SetColor(&rgb(colour, 1.0));
            self.gfx.icon(t, icon, icon_x, (r.top + r.bottom) / 2.0, icon_size, brush);
            if !label.is_empty() {
                self.gfx.text(t, label, &self.gfx.fonts.pill, rect(text_x, r.top, r.right - h / 2.0, r.bottom), brush);
            }
        };
        content(0xFFFFFF);
        if fill_x > r.left {
            // The filled part: the same pill clipped to the level, then the icon/label in black.
            t.PushAxisAlignedClip(&rect(r.left, r.top, fill_x, r.bottom), D2D1_ANTIALIAS_MODE_ALIASED);
            self.brush.SetColor(&rgb(fill, 1.0));
            t.FillRoundedRectangle(&shape, brush);
            content(0x000000);
            t.PopAxisAlignedClip();
        }
    }

    /// The closed notch's slider: [pill with icon] 62%
    pub(crate) unsafe fn draw_hud(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
        let s = self.dpi_scale;
        let w = self.width.value * s;
        let h = self.height.value.min(HUD.1) * s;
        let cx = self.px_w as f32 / 2.0;
        let (left, right) = (cx - w / 2.0, cx + w / 2.0);
        let kind = self.hud_kind;
        let level = self.level(kind);
        let (fill, icon) = match kind {
            Hud::Volume if self.sound.muted => (0x8E8E93, Icon::Mute),
            Hud::Volume => (0xFFFFFF, Icon::Speaker),
            Hud::Brightness | Hud::Track => (BRIGHT_YELLOW, Icon::Sun),
        };
        let pill_h = 22.0 * s;
        let pill = rect(left + 10.0 * s, (h - pill_h) / 2.0, right - 50.0 * s, (h + pill_h) / 2.0);
        self.draw_pill(t, brush, pill, level, fill, icon, "", alpha);
        self.brush.SetColor(&rgb(0xFFFFFF, 1.0));
        self.brush.SetOpacity(alpha);
        let text = format!("{}%", (level * 100.0).round() as u32);
        self.gfx.text(t, &text, &self.gfx.fonts.percent, rect(right - 48.0 * s, 0.0, right - 12.0 * s, h), brush);
    }
}
