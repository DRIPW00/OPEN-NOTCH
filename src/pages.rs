//! The pages of the open card (Controls, Music, Shelf, Network, System) and sliding between them.

use super::*;

/// Where page `i` sits relative to the slide position `pos`, in pages, with the pages in a
/// ring of `count`: the shortest way round, in (-count/2, count/2].
pub(crate) fn ring_offset(i: usize, pos: f32, count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let n = count as f32;
    let d = (i as f32 - pos).rem_euclid(n);
    if d > n / 2.0 { d - n } else { d }
}

impl App {
    /// Controls page (design H7): Volume and Brightness pills, then the Output and Input
    /// pickers and mute; an open picker lists its devices below.
    pub(crate) unsafe fn draw_controls(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
        let s = self.dpi_scale;
        let cx = self.px_w as f32 / 2.0;
        let w_box = CARD_W - 2.0 * MUSIC_PAD;
        let ox = cx - w_box / 2.0 * s;
        let at = |x: f32| ox + x * s;
        let y = |v: f32| v * s;
        let mut hits: Vec<(D2D_RECT_F, Action)> = Vec::new();
        let hovered = |a: &Action| self.hover_action.as_ref() == Some(a);
        let set = |hex: u32, a: f32| {
            self.brush.SetColor(&rgb(hex, 1.0));
            self.brush.SetOpacity(a * alpha);
        };

        // ---- Sliders
        let pill_w = (w_box - 16.0) / 2.0;
        let (top, bottom) = (14.0, 58.0);
        let vol = rect(at(0.0), y(top), at(pill_w), y(bottom));
        let (vol_fill, vol_icon) = if self.sound.muted { (0x8E8E93, Icon::Mute) } else { (0xFFFFFF, Icon::Speaker) };
        self.draw_pill(t, brush, vol, self.sound.volume, vol_fill, vol_icon, "Volume", alpha);
        hits.push((vol, Action::Volume { x0: vol.left, x1: vol.right }));
        let vol_text = if self.sound.muted { "Muted".to_string() } else { format!("{}%", (self.sound.volume * 100.0).round() as u32) };
        set(0x98989F, 1.0);
        self.gfx.text(t, &vol_text, &self.gfx.fonts.time, rect(at(8.0), y(61.0), at(pill_w), y(76.0)), brush);

        let bri = rect(at(pill_w + 16.0), y(top), at(w_box), y(bottom));
        if self.bright_supported {
            self.draw_pill(t, brush, bri, self.level(Hud::Brightness), BRIGHT_YELLOW, Icon::Sun, "Brightness", alpha);
            hits.push((bri, Action::Brightness { x0: bri.left, x1: bri.right }));
            set(0x98989F, 1.0);
            let text = self.bright.map_or("…".to_string(), |b| format!("{b}%"));
            self.gfx.text(t, &text, &self.gfx.fonts.time, rect(at(pill_w + 24.0), y(61.0), at(w_box), y(76.0)), brush);
        } else {
            self.draw_pill(t, brush, bri, 0.0, BRIGHT_YELLOW, Icon::Sun, "Brightness", alpha * 0.4);
            set(0x6E6E73, 1.0);
            self.gfx.text(t, "Not adjustable on this display", &self.gfx.fonts.time, rect(at(pill_w + 24.0), y(61.0), at(w_box), y(76.0)), brush);
        }

        // ---- Output / Input pickers and mute
        let (row_top, row_bottom) = (86.0, 122.0);
        let picker_w = 230.0;
        let mut picker = |x: f32, input: bool, name: String| {
            let r = rect(at(x), y(row_top), at(x + picker_w), y(row_bottom));
            let action = Action::Picker { input };
            let open = self.picker == Some(input);
            set(if open || hovered(&action) { 0x2C2C2E } else { 0x1C1C1E }, 1.0);
            t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(18.0), radiusY: y(18.0) }, brush);
            let icon = if input { Icon::Mic } else { device_icon(&name) };
            set(0xFFFFFF, 1.0);
            self.gfx.icon(t, icon, at(x + 20.0), y((row_top + row_bottom) / 2.0), y(15.0), brush);
            set(0x6E6E73, 1.0);
            self.gfx.text(t, if input { "INPUT" } else { "OUTPUT" }, &self.gfx.fonts.kind, rect(at(x + 36.0), y(row_top + 3.0), at(x + picker_w - 28.0), y(row_top + 16.0)), brush);
            set(0xFFFFFF, 1.0);
            self.gfx.text(t, &name, &self.gfx.fonts.bubble_text, rect(at(x + 36.0), y(row_top + 14.0), at(x + picker_w - 28.0), y(row_bottom - 3.0)), brush);
            set(0x98989F, 1.0);
            self.gfx.icon(t, Icon::ChevronDown, at(x + picker_w - 16.0), y((row_top + row_bottom) / 2.0), y(12.0), brush);
            hits.push((r, action));
        };
        picker(0.0, false, self.sound.output_name());
        picker(picker_w + 8.0, true, self.sound.input_name());

        let mute = Action::Mute;
        let (mx, my) = (w_box - 18.0, (row_top + row_bottom) / 2.0);
        let muted = self.sound.muted;
        set(if muted { 0xFFFFFF } else if hovered(&mute) { 0x2C2C2E } else { 0x1C1C1E }, 1.0);
        t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(mx), Y: y(my) }, radiusX: y(18.0), radiusY: y(18.0) }, brush);
        set(if muted { 0x000000 } else { 0xFFFFFF }, 1.0);
        self.gfx.icon(t, Icon::Mute, at(mx), y(my), y(16.0), brush);
        hits.push((rect(at(mx - 18.0), y(my - 18.0), at(mx + 18.0), y(my + 18.0)), mute));

        // ---- Open device list
        if let (Some((input, devices, default)), Some(list_h)) = (self.picker_devices(), self.device_list_height()) {
            let list_top = row_bottom + 8.0;
            set(0x111113, 1.0);
            let panel = rect(at(0.0), y(list_top), at(w_box), y(list_top + list_h));
            t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: panel, radiusX: y(14.0), radiusY: y(14.0) }, brush);
            if devices.is_empty() {
                set(0x6E6E73, 1.0);
                let none = if input { "No microphones found" } else { "No output devices found" };
                self.gfx.text(t, none, &self.gfx.fonts.bubble_text, rect(at(16.0), y(list_top + 6.0), at(w_box), y(list_top + 40.0)), brush);
            }
            for (i, device) in devices.iter().enumerate() {
                let row_y = list_top + 6.0 + i as f32 * 34.0;
                let r = rect(at(6.0), y(row_y), at(w_box - 6.0), y(row_y + 34.0));
                let action = Action::Device { id: device.id.clone(), input };
                let current = default.as_deref() == Some(device.id.as_str());
                if current || hovered(&action) {
                    set(if hovered(&action) { 0x2C2C2E } else { 0x1C1C1E }, 1.0);
                    t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(10.0), radiusY: y(10.0) }, brush);
                }
                let mid = row_y + 17.0;
                set(0xFFFFFF, 1.0);
                let icon = if input { Icon::Mic } else { device_icon(&device.name) };
                self.gfx.icon(t, icon, at(24.0), y(mid), y(14.0), brush);
                self.gfx.text(t, &device.name, &self.gfx.fonts.bubble_text, rect(at(42.0), y(row_y), at(w_box - 40.0), y(row_y + 34.0)), brush);
                if current {
                    set(CAMERA_GREEN, 1.0);
                    self.gfx.icon(t, Icon::Check, at(w_box - 24.0), y(mid), y(14.0), brush);
                }
                hits.push((r, action));
            }
        }
        hits
    }

    /// Album art in a rounded square, or a music-note placeholder.
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn draw_art(
        &self,
        t: &ID2D1RenderTarget,
        brush: &ID2D1Brush,
        session: &Session,
        x: f32,
        y: f32,
        size: f32,
        radius: f32,
        alpha: f32,
        big: bool,
    ) {
        let image = session
            .art
            .as_ref()
            .and_then(|a| self.art.get(&a.key))
            .map(|(b, small)| if big { b } else { small });
        if let Some(image) = image {
            gfx::rounded_image(t, image, x, y, size, radius, alpha);
        } else {
            self.brush.SetColor(&rgb(0x27272A, 1.0));
            self.brush.SetOpacity(alpha);
            let r = D2D1_ROUNDED_RECT { rect: rect(x, y, x + size, y + size), radiusX: radius, radiusY: radius };
            t.FillRoundedRectangle(&r, brush);
            self.brush.SetColor(&rgb(0x71717A, 1.0));
            self.gfx.icon(t, Icon::Note, x + size / 2.0, y + size / 2.0, size * 0.5, brush);
        }
    }

    /// The open card: the pages side by side, slid to `page_pos`, plus the side click
    /// strips and the page dots at the bottom.
    pub(crate) unsafe fn draw_pages(&mut self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
        let s = self.dpi_scale;
        let cx = self.px_w as f32 / 2.0;
        let card_w = CARD_W * s;
        let pos = self.page_pos.value;
        let pages = self.pages();
        let count = pages.len();
        let mut hits = Vec::new();

        if self.in_settings {
            hits.extend(self.draw_settings(t, brush, alpha));
        }
        let count = if self.in_settings { 0 } else { count };
        for (i, page) in pages.iter().enumerate().take(count) {
            let offset = ring_offset(i, pos, count) * card_w;
            if offset.abs() >= card_w {
                continue; // off-screen
            }
            t.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: offset, M32: 0.0 });
            let page_hits = match page {
                Page::Home => self.draw_idle_open(t, brush, alpha),
                Page::Music => self.draw_music(t, brush, alpha),
                Page::Shelf => self.draw_shelf(t, brush, alpha),
                Page::Controls => self.draw_controls(t, brush, alpha),
                Page::Network => self.draw_network(t, brush, alpha),
                Page::System => self.draw_system(t, brush, alpha),
            };
            if ring_offset(i, pos, count).abs() < 0.001 {
                hits.extend(page_hits); // only the settled page is clickable
            }
        }
        t.SetTransform(&Matrix3x2::identity());

        // Pin, in the card's top-right corner (above the right-hand page strip).
        {
            let (px, py) = (cx + card_w / 2.0 - 13.0 * s, 13.0 * s);
            let pin = Action::Pin;
            let hot = self.hover_action.as_ref() == Some(&pin);
            // Outline pin (grey, white on hover); solid white pin while pinned.
            self.brush.SetColor(&rgb(if self.pinned || hot { 0xFFFFFF } else { 0x636366 }, 1.0));
            self.brush.SetOpacity(alpha);
            let icon = if self.pinned { Icon::PinSolid } else { Icon::Pin };
            self.gfx.icon(t, icon, px, py, 13.0 * s, brush);
            // Listed first, so it wins over the page strip underneath it.
            hits.insert(0, (rect(px - 12.0 * s, 0.0, px + 12.0 * s, py + 12.0 * s), pin));
        }

        // Gear, in the top-left corner, on Home only (fades with the slide).
        let home_at = ring_offset(0, pos, count.max(1));
        let home_near = if self.in_settings { 0.0 } else { (1.0 - home_at.abs()).clamp(0.0, 1.0) };
        if home_near > 0.01 {
            // Inset past the left page strip, so reaching for it never switches pages.
            let (gx, gy) = (cx - card_w / 2.0 + (SIDE + 22.0) * s, 15.0 * s);
            let gear = Action::OpenSettings;
            let hot = self.hover_action.as_ref() == Some(&gear);
            self.brush.SetColor(&rgb(if hot { 0xFFFFFF } else { 0x636366 }, 1.0));
            self.brush.SetOpacity(alpha * home_near);
            self.gfx.icon(t, Icon::Gear, gx, gy, 13.0 * s, brush);
            if home_at.abs() < 0.001 {
                hits.insert(0, (rect(gx - 12.0 * s, 0.0, gx + 12.0 * s, gy + 12.0 * s), gear));
            }
        }

        if count > 1 {
            let h = self.height.value * s;
            let (left, right) = (cx - card_w / 2.0, cx + card_w / 2.0);
            let set = |hex: u32, a: f32| {
                self.brush.SetColor(&rgb(hex, 1.0));
                self.brush.SetOpacity(a * alpha);
            };
            // Side strips with a faint chevron; brighter when hovered.
            let strips = [
                (Action::PagePrev, rect(left, 0.0, left + SIDE * s, h), Icon::ChevronLeft),
                (Action::PageNext, rect(right - SIDE * s, 0.0, right, h), Icon::ChevronRight),
            ];
            for (action, r, icon) in strips {
                let hot = self.hover_action.as_ref() == Some(&action);
                set(if hot { 0xFFFFFF } else { 0x636366 }, if hot { 1.0 } else { 0.8 });
                self.gfx.icon(t, icon, (r.left + r.right) / 2.0, h / 2.0, 14.0 * s, brush);
                hits.push((r, action));
            }
            // Page dots: the current page is a wider white pill (follows the slide).
            let (dot, wide, gap) = (5.0 * s, 16.0 * s, 6.0 * s);
            let widths: Vec<f32> = (0..count).map(|i| dot + (wide - dot) * (1.0 - ring_offset(i, pos, count).abs()).clamp(0.0, 1.0)).collect();
            let total: f32 = widths.iter().sum::<f32>() + gap * (count - 1) as f32;
            let mut x = cx - total / 2.0;
            let y = h - 10.0 * s;
            for (i, w) in widths.iter().enumerate() {
                let near = (1.0 - ring_offset(i, pos, count).abs()).clamp(0.0, 1.0);
                set(if near > 0.5 { 0xFFFFFF } else { 0x636366 }, 1.0);
                let r = D2D1_ROUNDED_RECT { rect: rect(x, y - dot / 2.0, x + w, y + dot / 2.0), radiusX: dot / 2.0, radiusY: dot / 2.0 };
                t.FillRoundedRectangle(&r, brush);
                x += w + gap;
            }
        }

        if alpha > 0.9 && self.hovered {
            self.hits = hits;
        }
    }

    /// Music page: source tabs, art + song + seek bar, then the controls (from the design).
    pub(crate) unsafe fn draw_music(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
        let s = self.dpi_scale;
        let cx = self.px_w as f32 / 2.0;

        let Some(session) = self.current().filter(|s| s.status != Status::Stopped).cloned() else {
            return Vec::new();
        };
        // Content box: the full card width minus the side strips and a margin.
        // Everything below is in design pixels × s from its top-left.
        let w_box = CARD_W - 2.0 * MUSIC_PAD;
        let ox = cx - w_box / 2.0 * s;
        let at = |x: f32| ox + x * s;
        let y = |v: f32| v * s;
        let mut hits: Vec<(D2D_RECT_F, Action)> = Vec::new();
        let hovered = |a: &Action| self.hover_action.as_ref() == Some(a);
        let set = |hex: u32, a: f32| {
            self.brush.SetColor(&rgb(hex, 1.0));
            self.brush.SetOpacity(a * alpha);
        };

        // ---- Tabs: the shown app, plus other apps that are playing (greyed out). Those that
        // don't fit are summed up in a "+N" pill that opens the first of them.
        let tabs = self.tabs();
        let font_of = |active: bool| if active { &self.gfx.fonts.tab_active } else { &self.gfx.fonts.tab };
        let widths: Vec<f32> = tabs.iter().enumerate().map(|(i, tab)| 32.0 + self.gfx.measure(&tab.app_name, font_of(i == 0)) / s + 6.0).collect();
        let shown = fit_tabs(&widths, w_box + 6.0, MORE_TABS_W);
        let mut tab_x = 0.0;
        for (i, tab) in tabs.iter().take(shown).enumerate() {
            let active = i == 0;
            let font = font_of(active);
            let tab_w = widths[i] - 6.0;
            let r = rect(at(tab_x), y(14.0), at(tab_x + tab_w), y(40.0));
            let action = Action::Tab(tab.id.clone());
            let bg = if active { 0xFFFFFF } else if hovered(&action) { 0x2A2A2E } else { 0x1C1C1F };
            set(bg, 1.0);
            t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(13.0), radiusY: y(13.0) }, brush);
            // Green dot = this app is playing right now, whether or not it's the selected tab.
            let dot = if tab.status == Status::Playing { 0x22C55E } else { 0x71717A };
            set(dot, 1.0);
            let dot_c = Vector2 { X: at(tab_x + 13.0), Y: y(27.0) };
            t.FillEllipse(&D2D1_ELLIPSE { point: dot_c, radiusX: y(3.0), radiusY: y(3.0) }, brush);
            set(if active { 0x000000 } else { 0xA1A1AA }, 1.0);
            self.gfx.text(t, &tab.app_name, font, rect(at(tab_x + 22.0), y(14.0), at(tab_x + tab_w), y(40.0)), brush);
            if !active {
                hits.push((r, action));
            }
            tab_x += tab_w + 6.0;
        }
        if let Some(first_hidden) = tabs.get(shown) {
            let r = rect(at(tab_x), y(14.0), at(tab_x + MORE_TABS_W - 6.0), y(40.0));
            let action = Action::Tab(first_hidden.id.clone());
            set(if hovered(&action) { 0x2A2A2E } else { 0x1C1C1F }, 1.0);
            t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(13.0), radiusY: y(13.0) }, brush);
            set(0xA1A1AA, 1.0);
            let more = format!("+{}", tabs.len() - shown);
            let text_w = self.gfx.measure(&more, &self.gfx.fonts.tab) / s;
            let left = tab_x + (MORE_TABS_W - 6.0 - text_w) / 2.0;
            self.gfx.text(t, &more, &self.gfx.fonts.tab, rect(at(left), y(14.0), at(tab_x + MORE_TABS_W), y(40.0)), brush);
            hits.push((r, action));
        }

        // ---- Art, title, artist
        self.draw_art(t, brush, &session, at(0.0), y(52.0), y(68.0), y(14.0), alpha, true);
        // Waveform on the right, above the end of the progress bar; text stops short of it.
        self.draw_open_waveform(t, brush, at(w_box), y(86.0), alpha);
        let text_right = w_box - 40.0;
        set(0xFFFFFF, 1.0);
        self.gfx.text(t, &session.title, &self.gfx.fonts.title, rect(at(84.0), y(54.0), at(text_right), y(76.0)), brush);
        match &self.notice {
            Some(notice) => {
                set(0xFBBF24, 1.0); // amber: a short notice instead of the artist
                self.gfx.text(t, notice, &self.gfx.fonts.time, rect(at(84.0), y(77.0), at(text_right), y(95.0)), brush);
            }
            None => {
                set(0xA1A1AA, 1.0);
                self.gfx.text(t, &session.artist, &self.gfx.fonts.artist, rect(at(84.0), y(77.0), at(text_right), y(95.0)), brush);
            }
        }

        // ---- Seek bar with elapsed time and time left. Live streams have no length: no bar.
        if session.duration > 0.0 {
            let position = session.position_now();
            let progress = (position / session.duration).clamp(0.0, 1.0) as f32;
            let (bar_x0, bar_x1, bar_y) = (128.0, w_box - 48.0, 112.0);
            set(0xA1A1AA, 1.0);
            self.gfx.text(t, &format_time(position), &self.gfx.fonts.time, rect(at(84.0), y(104.0), at(124.0), y(120.0)), brush);
            self.gfx.text(t, &time_left(position, session.duration), &self.gfx.fonts.time_right, rect(at(w_box - 44.0), y(104.0), at(w_box), y(120.0)), brush);
            set(0x3F3F46, 1.0);
            let track = rect(at(bar_x0), y(bar_y - 2.0), at(bar_x1), y(bar_y + 2.0));
            t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: track, radiusX: y(2.0), radiusY: y(2.0) }, brush);
            let fill_x = bar_x0 + (bar_x1 - bar_x0) * progress;
            set(0xFFFFFF, 1.0);
            if progress > 0.0 {
                let fill = rect(at(bar_x0), y(bar_y - 2.0), at(fill_x), y(bar_y + 2.0));
                t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: fill, radiusX: y(2.0), radiusY: y(2.0) }, brush);
            }
            // Knob, as in the design. Draggable only if the app allows seeking.
            let seek = Action::Seek { x0: at(bar_x0), x1: at(bar_x1) };
            let knob = if session.can_seek && hovered(&seek) { 6.0 } else { 5.0 };
            t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(fill_x), Y: y(bar_y) }, radiusX: y(knob), radiusY: y(knob) }, brush);
            if session.can_seek {
                hits.push((rect(at(bar_x0 - 6.0), y(bar_y - 9.0), at(bar_x1 + 6.0), y(bar_y + 9.0)), seek));
            }
        }

        // ---- Controls: shuffle, previous, play/pause, next, repeat
        let cy = 154.0;
        // Five controls spread evenly across the content box.
        let col = |f: f32| w_box * f;
        let (x_shuffle, x_prev, x_play, x_next, x_repeat) = (col(0.09), col(0.3), col(0.5), col(0.7), col(0.91));
        let mut button = |x: f32, size: f32, icon: Icon, icon_size: f32, action: Action, enabled: bool, on: Option<bool>| {
            let r = rect(at(x - size / 2.0), y(cy - size / 2.0), at(x + size / 2.0), y(cy + size / 2.0));
            if enabled && hovered(&action) {
                set(0xFFFFFF, 0.12);
                t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(x), Y: y(cy) }, radiusX: y(size / 2.0), radiusY: y(size / 2.0) }, brush);
            }
            let (colour, a) = match (enabled, on) {
                (false, _) => (0xFFFFFF, 0.3),
                (true, Some(false)) => (0xA1A1AA, 1.0), // shuffle/repeat off
                _ => (0xFFFFFF, 1.0),
            };
            set(colour, a);
            self.gfx.icon(t, icon, at(x), y(cy), y(icon_size), brush);
            if on == Some(true) {
                // Small dot under an active shuffle/repeat, like Apple's players.
                t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(x), Y: y(cy + 13.0) }, radiusX: y(1.5), radiusY: y(1.5) }, brush);
            }
            if enabled {
                hits.push((r, action));
            }
        };
        // Shuffle/repeat only for apps that report them to Windows (no dead buttons).
        if let Some(on) = session.shuffle {
            button(x_shuffle, 34.0, Icon::Shuffle, 16.0, Action::Shuffle, true, Some(on));
        }
        button(x_prev, 36.0, Icon::Previous, 20.0, Action::Previous, session.can_prev, None);
        button(x_next, 36.0, Icon::Next, 20.0, Action::Next, session.can_next, None);
        if let Some(mode) = session.repeat {
            button(x_repeat, 34.0, Icon::Repeat, 16.0, Action::Repeat, true, Some(mode != Repeat::Off));
        }
        if session.repeat == Some(Repeat::One) {
            set(0xFFFFFF, 1.0);
            self.gfx.text(t, "1", &self.gfx.fonts.badge, rect(at(x_repeat - 4.0), y(cy - 4.0), at(x_repeat + 4.0), y(cy + 4.0)), brush);
        }

        // Play/pause: white circle, black icon.
        let pp = Action::PlayPause;
        set(if hovered(&pp) { 0xE4E4E7 } else { 0xFFFFFF }, if session.can_play_pause { 1.0 } else { 0.3 });
        t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(x_play), Y: y(cy) }, radiusX: y(22.0), radiusY: y(22.0) }, brush);
        set(0x000000, 1.0);
        let icon = if session.status == Status::Playing { Icon::Pause } else { Icon::Play };
        self.gfx.icon(t, icon, at(x_play), y(cy), y(20.0), brush);
        if session.can_play_pause {
            hits.push((rect(at(x_play - 22.0), y(cy - 22.0), at(x_play + 22.0), y(cy + 22.0)), pp));
        }
        hits
    }

    /// A file's thumbnail in a rounded square (dark tile behind icons; the extension if
    /// there's no preview yet).
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn draw_file_thumb(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, path: &str, x: f32, y: f32, size: f32, radius: f32, alpha: f32) {
        self.brush.SetColor(&rgb(0x1C1C1E, 1.0));
        self.brush.SetOpacity(alpha);
        t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(x, y, x + size, y + size), radiusX: radius, radiusY: radius }, brush);
        if let Some(image) = self.shelf_art.get(path) {
            gfx::rounded_image(t, image, x, y, size, radius, alpha);
        } else if size > 24.0 * self.dpi_scale {
            let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
            self.brush.SetColor(&rgb(0x98989F, 1.0));
            self.gfx.text(t, &ext, &self.gfx.fonts.label_center, rect(x, y, x + size, y + size), brush);
        }
    }

    /// Files dragged over the notch: a dashed drop zone (design S1).
    pub(crate) unsafe fn draw_drop_zone(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
        let s = self.dpi_scale;
        let cx = self.px_w as f32 / 2.0;
        let w = self.width.value * s;
        let h = self.height.value * s;
        let zone = rect(cx - w / 2.0 + 24.0 * s, 18.0 * s, cx + w / 2.0 - 24.0 * s, (h - 18.0 * s).max(19.0 * s));
        self.brush.SetOpacity(alpha * 0.12);
        self.brush.SetColor(&rgb(SHELF_BLUE, 1.0));
        t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: zone, radiusX: 18.0 * s, radiusY: 18.0 * s }, brush);
        self.brush.SetOpacity(alpha);
        self.gfx.dashed_round_rect(t, zone, 18.0 * s, 2.0 * s, brush);
        let mid = (zone.top + zone.bottom) / 2.0;
        self.gfx.icon(t, Icon::ArrowDown, cx, mid - 22.0 * s, 22.0 * s, brush);
        self.brush.SetColor(&rgb(0xFFFFFF, 1.0));
        self.gfx.text(t, "Drop to keep on the shelf", &self.gfx.fonts.ring_value, rect(zone.left, mid - 6.0 * s, zone.right, mid + 12.0 * s), brush);
        self.brush.SetColor(&rgb(0x98989F, 1.0));
        self.gfx.text(t, "Drag it back out whenever you need it", &self.gfx.fonts.small_center, rect(zone.left, mid + 12.0 * s, zone.right, mid + 28.0 * s), brush);
    }

    /// Shelf page (design S1): "SHELF · N FILES" + Clear all, then a row of file tiles and
    /// a dashed slot inviting more drops.
    pub(crate) unsafe fn draw_shelf(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
        let s = self.dpi_scale;
        let cx = self.px_w as f32 / 2.0;
        let w_box = CARD_W - 2.0 * MUSIC_PAD;
        let ox = cx - w_box / 2.0 * s;
        let at = |x: f32| ox + x * s;
        let y = |v: f32| v * s;
        let mut hits: Vec<(D2D_RECT_F, Action)> = Vec::new();
        let hovered = |a: &Action| self.hover_action.as_ref() == Some(a);
        let set = |hex: u32, a: f32| {
            self.brush.SetColor(&rgb(hex, 1.0));
            self.brush.SetOpacity(a * alpha);
        };

        let n = self.shelf.len();
        set(0x6E6E73, 1.0);
        let title = if n == 1 { "SHELF · 1 FILE".to_string() } else { format!("SHELF · {n} FILES") };
        self.gfx.text(t, &title, &self.gfx.fonts.kind, rect(at(0.0), y(12.0), at(300.0), y(30.0)), brush);
        let clear = Action::ShelfClear;
        set(if hovered(&clear) { 0xFFFFFF } else { 0x98989F }, 1.0);
        self.gfx.text(t, "Clear all", &self.gfx.fonts.time_right, rect(at(w_box - 90.0), y(12.0), at(w_box), y(30.0)), brush);
        let clear_w = self.gfx.measure("Clear all", &self.gfx.fonts.time_right) / s;
        self.gfx.icon(t, Icon::Close, at(w_box - clear_w - 9.0), y(21.0), y(10.0), brush);
        hits.push((rect(at(w_box - clear_w - 18.0), y(10.0), at(w_box), y(32.0)), clear));

        // Tiles: 80 wide, 12 apart; the thumbnail is 56 square, the name under it.
        let (tile_w, gap, thumb, top) = (80.0, 12.0, 56.0, 38.0);
        let start = self.shelf_scroll.min(n.saturating_sub(SHELF_TILES));
        for (slot, (i, path)) in self.shelf.iter().enumerate().skip(start).take(SHELF_TILES).enumerate() {
            let x = slot as f32 * (tile_w + gap);
            let tx = x + (tile_w - thumb) / 2.0;
            let item = Action::ShelfItem(i);
            let remove = Action::ShelfRemove(i);
            let hot = hovered(&item) || hovered(&remove);
            self.draw_file_thumb(t, brush, path, at(tx), y(top), y(thumb), y(12.0), alpha);
            if hot {
                set(0xFFFFFF, 0.25);
                t.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(at(tx), y(top), at(tx + thumb), y(top + thumb)), radiusX: y(12.0), radiusY: y(12.0) }, brush, y(1.5), None);
                // Remove button on the thumbnail's corner.
                let (rx, ry) = (tx + thumb - 2.0, top + 2.0);
                set(if hovered(&remove) { 0x636366 } else { 0x3A3A3C }, 1.0);
                t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(rx), Y: y(ry) }, radiusX: y(9.0), radiusY: y(9.0) }, brush);
                set(0xFFFFFF, 1.0);
                self.gfx.icon(t, Icon::Close, at(rx), y(ry), y(9.0), brush);
                hits.push((rect(at(rx - 10.0), y(ry - 10.0), at(rx + 10.0), y(ry + 10.0)), remove));
            }
            set(0xFFFFFF, 1.0);
            self.gfx.text(t, &shelf::file_name(path), &self.gfx.fonts.small_center, rect(at(x), y(top + thumb + 4.0), at(x + tile_w), y(top + thumb + 20.0)), brush);
            hits.push((rect(at(x), y(top), at(x + tile_w), y(top + thumb + 20.0)), item));
        }
        // Drop hint (or how many more there are, if the row scrolls).
        let slot_x = SHELF_TILES as f32 * (tile_w + gap) + (tile_w - thumb) / 2.0;
        let hint = rect(at(slot_x), y(top), at(slot_x + thumb), y(top + thumb));
        set(0x3A3A3C, 1.0);
        self.gfx.dashed_round_rect(t, hint, y(12.0), y(1.5), brush);
        let more = n.saturating_sub(start + SHELF_TILES);
        if more > 0 {
            set(0x98989F, 1.0);
            self.gfx.text(t, &format!("+{more}"), &self.gfx.fonts.ring_value, hint, brush);
        } else {
            set(0x6E6E73, 1.0);
            self.gfx.icon(t, Icon::ArrowDown, at(slot_x + thumb / 2.0), y(top + thumb / 2.0), y(16.0), brush);
        }
        hits
    }

    /// Network page: Wi-Fi and Bluetooth tiles; clicking one lists networks / devices below.
    pub(crate) unsafe fn draw_network(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
        let s = self.dpi_scale;
        let cx = self.px_w as f32 / 2.0;
        let w_box = CARD_W - 2.0 * MUSIC_PAD;
        let ox = cx - w_box / 2.0 * s;
        let at = |x: f32| ox + x * s;
        let y = |v: f32| v * s;
        let mut hits: Vec<(D2D_RECT_F, Action)> = Vec::new();
        let hovered = |a: &Action| self.hover_action.as_ref() == Some(a);
        let set = |hex: u32, a: f32| {
            self.brush.SetColor(&rgb(hex, 1.0));
            self.brush.SetOpacity(a * alpha);
        };

        // ---- Tiles
        let (top, bottom) = (14.0, 70.0);
        let tile_w = (w_box - 12.0) / 2.0;
        let wifi_on = self.radio_state.wifi.unwrap_or(self.wifi_state.available);
        let bt_on = self.radio_state.bluetooth.unwrap_or(self.bt_state.available);
        let wifi_sub = if !wifi_on || !self.wifi_state.available {
            "Off".to_string()
        } else if let Some(j) = &self.wifi_state.joining {
            format!("Joining {j}…")
        } else {
            self.wifi_state.connected().map_or("Not connected".to_string(), |n| n.ssid.clone())
        };
        let connected_bt: Vec<&bluetooth::Device> = self.bt_state.devices.iter().filter(|d| d.connected).collect();
        let bt_sub = if !bt_on || !self.bt_state.available {
            "Off".to_string()
        } else {
            match connected_bt.len() {
                0 => "Not connected".to_string(),
                1 => connected_bt[0].name.clone(),
                n => format!("{n} devices connected"),
            }
        };
        let tiles = [
            (0.0, false, Icon::Wifi, "Wi-Fi", wifi_sub, wifi_on, self.radio_state.wifi.is_some()),
            (tile_w + 12.0, true, Icon::Bluetooth, "Bluetooth", bt_sub, bt_on, self.radio_state.bluetooth.is_some()),
        ];
        for (x, bt, icon, title, sub, on, switchable) in tiles {
            let r = rect(at(x), y(top), at(x + tile_w), y(bottom));
            let action = Action::NetList { bt };
            let open = self.net_list == Some(bt);
            set(if open || hovered(&action) { 0x2C2C2E } else { 0x1C1C1E }, 1.0);
            t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(18.0), radiusY: y(18.0) }, brush);
            let mid = (top + bottom) / 2.0;
            set(if on { WIFI_BLUE } else { 0x3A3A3C }, 1.0);
            t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(x + 30.0), Y: y(mid) }, radiusX: y(17.0), radiusY: y(17.0) }, brush);
            set(0xFFFFFF, 1.0);
            self.gfx.icon(t, icon, at(x + 30.0), y(mid), y(18.0), brush);
            let text_right = x + tile_w - if switchable { 60.0 } else { 30.0 };
            self.gfx.text(t, title, &self.gfx.fonts.pill, rect(at(x + 56.0), y(mid - 18.0), at(text_right), y(mid)), brush);
            set(0x98989F, 1.0);
            self.gfx.text(t, &sub, &self.gfx.fonts.bubble_text, rect(at(x + 56.0), y(mid), at(text_right), y(mid + 17.0)), brush);
            // On/off switch at the tile's right (its own click target, checked first).
            if switchable {
                let (sw, sh) = (36.0, 22.0);
                let sx = x + tile_w - 14.0 - sw;
                let track = rect(at(sx), y(mid - sh / 2.0), at(sx + sw), y(mid + sh / 2.0));
                let switch = Action::Radio { bt };
                set(if on { CAMERA_GREEN } else { 0x48484A }, if hovered(&switch) { 0.85 } else { 1.0 });
                t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: track, radiusX: y(sh / 2.0), radiusY: y(sh / 2.0) }, brush);
                let knob_x = if on { sx + sw - sh / 2.0 } else { sx + sh / 2.0 };
                set(0xFFFFFF, 1.0);
                t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(knob_x), Y: y(mid) }, radiusX: y(sh / 2.0 - 2.5), radiusY: y(sh / 2.0 - 2.5) }, brush);
                hits.push((rect(at(sx - 6.0), y(top), at(x + tile_w), y(bottom)), switch));
            } else {
                set(0x98989F, 1.0);
                self.gfx.icon(t, Icon::ChevronDown, at(x + tile_w - 18.0), y(mid), y(12.0), brush);
            }
            hits.push((r, action));
        }

        // ---- Open list
        let (Some(bt), Some(list_h)) = (self.net_list, self.net_list_height()) else { return hits };
        let list_top = bottom + 8.0;
        set(0x111113, 1.0);
        let panel = rect(at(0.0), y(list_top), at(w_box), y(list_top + list_h));
        t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: panel, radiusX: y(14.0), radiusY: y(14.0) }, brush);

        // Header: title, then refresh (Wi-Fi rescans; Bluetooth re-reads).
        set(0x6E6E73, 1.0);
        let title = if bt { "PAIRED DEVICES" } else { "NETWORKS" };
        self.gfx.text(t, title, &self.gfx.fonts.kind, rect(at(16.0), y(list_top + 6.0), at(200.0), y(list_top + LIST_HEADER)), brush);
        let refresh = Action::WifiRefresh;
        let (rx, ry) = (w_box - 22.0, list_top + 4.0 + (LIST_HEADER - 4.0) / 2.0);
        if self.wifi_state.scanning && !bt {
            set(0x98989F, 1.0);
            self.gfx.text(t, "Scanning…", &self.gfx.fonts.time_right, rect(at(w_box - 160.0), y(list_top + 6.0), at(rx - 14.0), y(list_top + LIST_HEADER)), brush);
        }
        if hovered(&refresh) {
            set(0x2C2C2E, 1.0);
            t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(rx), Y: y(ry) }, radiusX: y(13.0), radiusY: y(13.0) }, brush);
        }
        set(0xFFFFFF, if self.wifi_state.scanning && !bt { 0.4 } else { 1.0 });
        self.gfx.icon(t, Icon::Refresh, at(rx), y(ry), y(14.0), brush);
        hits.push((rect(at(rx - 14.0), y(ry - 14.0), at(rx + 14.0), y(ry + 14.0)), refresh));

        let rows_top = list_top + LIST_HEADER;
        // Rows below an open password row move down to make room for it.
        let pw_index = self.pw_ssid.as_ref().and_then(|ssid| self.wifi_state.networks.iter().take(MAX_DEVICES).position(|n| &n.ssid == ssid));
        let row_rect = |i: usize| {
            let extra = if !bt && pw_index.is_some_and(|p| i > p) { PASSWORD_ROW } else { 0.0 };
            let row_y = rows_top + i as f32 * 34.0 + extra;
            (row_y, rect(at(6.0), y(row_y), at(w_box - 6.0), y(row_y + 34.0)))
        };
        let empty = |text: &str| {
            set(0x6E6E73, 1.0);
            self.gfx.text(t, text, &self.gfx.fonts.bubble_text, rect(at(16.0), y(rows_top), at(w_box), y(rows_top + 34.0)), brush);
        };
        if bt {
            if !bt_on || !self.bt_state.available {
                empty("Bluetooth is off");
            } else if self.bt_state.devices.is_empty() {
                empty("No paired devices");
            }
            for (i, device) in self.bt_state.devices.iter().filter(|_| bt_on).take(MAX_DEVICES).enumerate() {
                let (row_y, r) = row_rect(i);
                let action = Action::Bt(device.name.clone());
                if device.connected || hovered(&action) {
                    set(if hovered(&action) { 0x2C2C2E } else { 0x1C1C1E }, 1.0);
                    t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(10.0), radiusY: y(10.0) }, brush);
                }
                let mid = row_y + 17.0;
                let icon = match device.kind {
                    bluetooth::Kind::Audio => Icon::Headphones,
                    _ => Icon::Bluetooth,
                };
                set(0xFFFFFF, 1.0);
                self.gfx.icon(t, icon, at(24.0), y(mid), y(14.0), brush);
                self.gfx.text(t, &device.name, &self.gfx.fonts.bubble_text, rect(at(42.0), y(row_y), at(w_box - 120.0), y(row_y + 34.0)), brush);
                let busy = self.bt_state.busy.as_deref() == Some(device.name.as_str());
                let (status, colour) = if busy {
                    (if device.connected { "Disconnecting…" } else { "Connecting…" }, 0x98989F)
                } else if device.connected {
                    ("Connected", CAMERA_GREEN)
                } else if device.kind != bluetooth::Kind::Audio && hovered(&action) {
                    ("Connects on its own", 0x6E6E73) // mice, keyboards… reconnect by themselves
                } else {
                    ("", 0)
                };
                set(colour, 1.0);
                self.gfx.text(t, status, &self.gfx.fonts.time_right, rect(at(w_box - 130.0), y(row_y), at(w_box - 18.0), y(row_y + 34.0)), brush);
                hits.push((r, action));
            }
        } else {
            if !wifi_on || !self.wifi_state.available {
                empty("Wi-Fi is off");
            } else if self.wifi_state.networks.is_empty() {
                empty(if self.wifi_state.scanning { "Looking for networks…" } else { "No networks found" });
            }
            for (i, network) in self.wifi_state.networks.iter().filter(|_| wifi_on).take(MAX_DEVICES).enumerate() {
                let (row_y, r) = row_rect(i);
                let action = Action::Wifi(network.ssid.clone());
                if network.connected || hovered(&action) {
                    set(if hovered(&action) { 0x2C2C2E } else { 0x1C1C1E }, 1.0);
                    t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(10.0), radiusY: y(10.0) }, brush);
                }
                let mid = row_y + 17.0;
                // Weaker signal = dimmer icon.
                let strength = if network.signal >= 70 { 1.0 } else if network.signal >= 40 { 0.7 } else { 0.45 };
                set(0xFFFFFF, strength);
                self.gfx.icon(t, Icon::Wifi, at(24.0), y(mid), y(15.0), brush);
                set(0xFFFFFF, 1.0);
                self.gfx.text(t, &network.ssid, &self.gfx.fonts.bubble_text, rect(at(42.0), y(row_y), at(w_box - 120.0), y(row_y + 34.0)), brush);
                let joining = self.wifi_state.joining.as_deref() == Some(network.ssid.as_str());
                let failed = self.wifi_state.error.as_ref().filter(|(ssid, _)| ssid == &network.ssid && self.pw_ssid.as_ref() != Some(ssid));
                if joining {
                    set(0x98989F, 1.0);
                    self.gfx.text(t, "Joining…", &self.gfx.fonts.time_right, rect(at(w_box - 130.0), y(row_y), at(w_box - 40.0), y(row_y + 34.0)), brush);
                } else if let Some((_, message)) = failed {
                    set(DAY_RED, 1.0);
                    self.gfx.text(t, message, &self.gfx.fonts.time_right, rect(at(w_box - 250.0), y(row_y), at(w_box - 40.0), y(row_y + 34.0)), brush);
                } else if network.security == wifi::Security::Unsupported && hovered(&action) && !network.connected && network.profile.is_none() {
                    set(0x6E6E73, 1.0);
                    self.gfx.text(t, "Needs a work/school sign-in", &self.gfx.fonts.time_right, rect(at(w_box - 250.0), y(row_y), at(w_box - 40.0), y(row_y + 34.0)), brush);
                } else if network.connected {
                    set(CAMERA_GREEN, 1.0);
                    self.gfx.icon(t, Icon::Check, at(w_box - 48.0), y(mid), y(14.0), brush);
                }
                if network.secured {
                    set(0x98989F, 1.0);
                    self.gfx.icon(t, Icon::Lock, at(w_box - 24.0), y(mid), y(12.0), brush);
                }
                hits.push((r, action));

                // Password row for a new network: [ field  (Show) ] [Join]
                if self.pw_ssid.as_deref() == Some(network.ssid.as_str()) {
                    let top = row_y + 34.0 + 4.0;
                    let (fx0, fx1, fy0, fy1) = (42.0, w_box - 96.0, top, top + 34.0);
                    let field = rect(at(fx0), y(fy0), at(fx1), y(fy1));
                    set(0x1C1C1E, 1.0);
                    t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: field, radiusX: y(10.0), radiusY: y(10.0) }, brush);
                    set(if self.pw_focus { SHELF_BLUE } else { 0x3A3A3C }, 1.0);
                    t.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: field, radiusX: y(10.0), radiusY: y(10.0) }, brush, y(1.5), None);
                    let error = self.wifi_state.error.as_ref().filter(|(ssid, _)| ssid == &network.ssid).map(|(_, m)| m.clone());
                    let shown = if self.pw_show { self.pw_text.clone() } else { "•".repeat(self.pw_text.chars().count()) };
                    let text_r = rect(at(fx0 + 12.0), y(fy0), at(fx1 - 52.0), y(fy1));
                    if self.pw_text.is_empty() {
                        match &error {
                            Some(_) => set(DAY_RED, 1.0),
                            None => set(0x6E6E73, 1.0),
                        }
                        let placeholder = error.clone().unwrap_or_else(|| "Password".to_string());
                        self.gfx.text(t, &placeholder, &self.gfx.fonts.bubble_text, text_r, brush);
                    } else {
                        set(0xFFFFFF, 1.0);
                        self.gfx.text(t, &shown, &self.gfx.fonts.bubble_text, text_r, brush);
                    }
                    if self.pw_focus && self.caret_on {
                        let caret_x = (fx0 + 12.0 + self.gfx.measure(&shown, &self.gfx.fonts.bubble_text) / s + 1.0).min(fx1 - 54.0);
                        set(SHELF_BLUE, 1.0);
                        t.FillRectangle(&rect(at(caret_x), y(fy0 + 9.0), at(caret_x + 1.5), y(fy1 - 9.0)), brush);
                    }
                    hits.push((field, Action::PwField));
                    // Show / Hide
                    let show = Action::PwShow;
                    set(if hovered(&show) { 0xFFFFFF } else { 0x98989F }, 1.0);
                    let show_r = rect(at(fx1 - 50.0), y(fy0), at(fx1 - 10.0), y(fy1));
                    self.gfx.text(t, if self.pw_show { "Hide" } else { "Show" }, &self.gfx.fonts.time_right, show_r, brush);
                    hits.insert(hits.len() - 1, (show_r, show));
                    // Join
                    let join = Action::PwJoin;
                    let joining_now = self.wifi_state.joining.as_deref() == Some(network.ssid.as_str());
                    let ready = self.pw_text.chars().count() >= 8 && !joining_now;
                    let jr = rect(at(w_box - 86.0), y(fy0), at(w_box - 6.0), y(fy1));
                    set(if ready { SHELF_BLUE } else { 0x2C2C2E }, if ready && hovered(&join) { 0.85 } else { 1.0 });
                    t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: jr, radiusX: y(17.0), radiusY: y(17.0) }, brush);
                    set(if ready { 0xFFFFFF } else { 0x6E6E73 }, 1.0);
                    self.gfx.text(t, if joining_now { "Joining…" } else { "Join" }, &self.gfx.fonts.ring_value, jr, brush);
                    hits.push((jr, join));
                }
            }
        }
        hits
    }

    /// System page: CPU, RAM, GPU and SSD rings (like the battery ring), with details below.
    pub(crate) unsafe fn draw_system(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
        let s = self.dpi_scale;
        let cx = self.px_w as f32 / 2.0;
        let w_box = CARD_W - 2.0 * MUSIC_PAD;
        let ox = cx - w_box / 2.0 * s;
        let at = |x: f32| ox + x * s;
        let y = |v: f32| v * s;
        let gb = |bytes: u64| bytes as f64 / 1024f64.powi(3);
        let sys = &self.sys;

        let ram = (sys.ram_total > 0).then(|| sys.ram_used as f32 / sys.ram_total as f32);
        let ssd_used = (sys.disk_total > 0).then(|| 1.0 - sys.disk_free as f32 / sys.disk_total as f32);
        let columns: [(&str, u32, Option<f32>, String); 4] = [
            ("CPU", CPU_PINK, sys.cpu, if sys.cpu_name.is_empty() { "Processor".to_string() } else { sys.cpu_name.clone() }),
            ("RAM", RAM_LIME, ram, if sys.ram_total > 0 { format!("{:.1} / {:.0} GB", gb(sys.ram_used), gb(sys.ram_total)) } else { String::new() }),
            ("GPU", GPU_PURPLE, sys.gpu, if sys.gpu_name.is_empty() { "Graphics".to_string() } else { sys.gpu_name.clone() }),
            ("SSD", SSD_CYAN, ssd_used, if sys.disk_total > 0 { format!("{:.0} GB free", gb(sys.disk_free)) } else { String::new() }),
        ];
        let col_w = w_box / 4.0;
        let (ring_cy, r, width) = (44.0, 25.0, 5.0);
        for (i, (label, colour, value, detail)) in columns.iter().enumerate() {
            let x = col_w * (i as f32 + 0.5);
            self.brush.SetOpacity(alpha);
            self.brush.SetColor(&rgb(0x2C2C2E, 1.0));
            t.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(x), Y: y(ring_cy) }, radiusX: y(r), radiusY: y(r) }, brush, y(width), None);
            self.brush.SetColor(&rgb(*colour, 1.0));
            if let Some(v) = value {
                self.gfx.arc(t, at(x), y(ring_cy), y(r), y(width), *v, brush);
            }
            self.brush.SetColor(&rgb(0xFFFFFF, 1.0));
            let text = value.map_or("–".to_string(), |v| format!("{}%", (v * 100.0).round() as u32));
            self.gfx.text(t, &text, &self.gfx.fonts.ring_value, rect(at(x - r), y(ring_cy - 10.0), at(x + r), y(ring_cy + 10.0)), brush);
            self.brush.SetColor(&rgb(*colour, 1.0));
            self.gfx.text(t, label, &self.gfx.fonts.label_center, rect(at(x - col_w / 2.0), y(78.0), at(x + col_w / 2.0), y(92.0)), brush);
            self.brush.SetColor(&rgb(0x98989F, 1.0));
            self.gfx.text(t, detail, &self.gfx.fonts.small_center, rect(at(x - col_w / 2.0 + 4.0), y(92.0), at(x + col_w / 2.0 - 4.0), y(107.0)), brush);
        }
        Vec::new()
    }

    /// Slide to another page (wraps around), resizing the card to fit it.
    pub(crate) unsafe fn switch_page(&mut self, page: usize) {
        let count = self.page_count();
        self.slide_to(page % count.max(1), None);
    }

    /// One page forward (+1) or back (-1). The pages are a ring: forward from the last
    /// page slides on to Home, back from Home slides on to the last page.
    pub(crate) unsafe fn step_page(&mut self, dir: i32) {
        let count = self.page_count().max(1);
        let page = (self.page as i32 + dir).rem_euclid(count as i32) as usize;
        self.slide_to(page, Some(dir));
    }

    /// Slide to `page`, `dir` pages along the ring (or the shorter way round if None).
    pub(crate) unsafe fn slide_to(&mut self, page: usize, dir: Option<i32>) {
        let count = self.page_count();
        if page == self.page && ring_offset(page, self.page_pos.target, count).abs() < 0.001 {
            return;
        }
        let step = dir.unwrap_or_else(|| ring_offset(page, self.page_pos.target, count).round() as i32);
        self.page = page;
        self.picker = None;
        self.net_list = None;
        self.close_password();
        self.update_stats_wanted();
        if self.page_kind() == Page::Network {
            self.wifi.refresh_list();
            self.bt.refresh();
        }
        self.page_pos.set_target(self.page_pos.target + step as f32);
        self.last_page_switch = Instant::now();
        if self.prefs.last != self.page_kind() {
            self.prefs.last = self.page_kind();
            self.prefs.save();
        }
        self.hits.clear();
        self.set_hover_action(None);
        if self.hovered {
            self.set_expanded(true); // new height for this page
        } else {
            self.start_animation();
        }
    }
}

/// Width of the "+N" pill for source tabs that don't fit, gap included (design pixels).
const MORE_TABS_W: f32 = 36.0;

/// How many source tabs fit in `avail` (each width includes its gap), keeping room for the
/// "+N" pill while some are left over. The shown tab (the first) always stays.
fn fit_tabs(widths: &[f32], avail: f32, more_w: f32) -> usize {
    let mut x = 0.0;
    for (i, w) in widths.iter().enumerate() {
        let need = if i + 1 < widths.len() { w + more_w } else { *w };
        if i > 0 && x + need > avail {
            return i;
        }
        x += w;
    }
    widths.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    // R1.04: source tabs that don't fit leave room for a "+N" pill instead of vanishing.
    #[test]
    fn tabs_that_dont_fit_leave_room_for_more() {
        // Widths include the gap after each tab; the "+N" pill needs 30.
        assert_eq!(fit_tabs(&[60.0, 60.0], 200.0, 30.0), 2, "all fit: no pill needed");
        assert_eq!(fit_tabs(&[60.0, 60.0, 60.0, 60.0], 200.0, 30.0), 2, "3rd would leave no room for +2");
        assert_eq!(fit_tabs(&[60.0, 60.0, 60.0], 180.0, 30.0), 3, "the last one fits exactly: no pill");
        assert_eq!(fit_tabs(&[250.0, 60.0], 200.0, 30.0), 1, "the shown tab always stays");
        assert_eq!(fit_tabs(&[], 200.0, 30.0), 0);
    }

    // R1.04: time left, like the music apps.
    #[test]
    fn time_left_counts_down() {
        assert_eq!(time_left(30.0, 180.0), "-2:30");
        assert_eq!(time_left(180.0, 180.0), "-0:00");
        assert_eq!(time_left(200.0, 180.0), "-0:00", "past the end: never negative");
        assert_eq!(time_left(0.0, 3_725.0), "-1:02:05");
    }

    // One-way page loop: pages sit on a ring.
    #[test]
    fn ring_offset_wraps_the_short_way() {
        // 4 pages, sitting on page 3 (the last): page 0 (Home) is just ahead, not 3 back.
        assert_eq!(ring_offset(0, 3.0, 4), 1.0);
        assert_eq!(ring_offset(2, 3.0, 4), -1.0);
        // Sitting on Home: the last page is just behind.
        assert_eq!(ring_offset(3, 0.0, 4), -1.0);
        assert_eq!(ring_offset(1, 0.0, 4), 1.0);
        assert_eq!(ring_offset(1, 1.0, 4), 0.0);
        assert_eq!(ring_offset(0, 0.0, 0), 0.0); // no pages: no crash
    }

    #[test]
    fn last_to_home_slides_forward_smoothly() {
        // Slide position goes 3 -> 4 (past the end). Home must come in from the right and
        // the last page leave to the left, each moving one way only, never jumping.
        let (count, steps) = (4, 100);
        let mut last = (ring_offset(0, 3.0, count), ring_offset(3, 3.0, count));
        for k in 1..=steps {
            let pos = 3.0 + k as f32 / steps as f32;
            let now = (ring_offset(0, pos, count), ring_offset(3, pos, count));
            assert!(now.0 < last.0 && now.0 >= 0.0, "Home moves left into place");
            assert!(now.1 < last.1 && now.1 >= -1.0, "last page moves left out");
            assert!((last.0 - now.0) < 0.02 && (last.1 - now.1) < 0.02, "no jump");
            last = now;
        }
        assert_eq!(ring_offset(0, 4.0, count), 0.0); // Home exactly in place
    }

    #[test]
    fn home_to_last_slides_backward_smoothly() {
        let count = 5;
        let mut last = ring_offset(4, 0.0, count);
        for k in 1..=100 {
            let pos = -(k as f32) / 100.0;
            let now = ring_offset(4, pos, count);
            assert!(now > last && now <= 0.0 && now - last < 0.02);
            last = now;
        }
    }

    #[test]
    fn two_pages_still_slide() {
        // With 2 pages forward from page 1 goes on to page 0 from the right.
        assert_eq!(ring_offset(0, 1.0, 2), 1.0);
        assert!((ring_offset(0, 1.5, 2) - 0.5).abs() < 1e-6);
        assert!((ring_offset(1, 1.5, 2) + 0.5).abs() < 1e-6);
    }
}
