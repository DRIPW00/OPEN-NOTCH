//! The Settings card: General, Motion, Pages and About tabs.

use super::*;

impl App {
    pub(crate) fn settings_height(&self) -> f32 {
        match self.settings_tab {
            TAB_GENERAL => SETTINGS_CONTENT + self.general_rows().len() as f32 * SETTINGS_ROW + 8.0,
            TAB_MOTION => SETTINGS_CONTENT + 3.0 * SETTINGS_ROW + 8.0,
            TAB_PAGES => SETTINGS_TOP + (self.prefs.order.len() + 1) as f32 * SETTINGS_ROW + 8.0,
            _ => SETTINGS_CONTENT + 64.0,
        }
    }

    /// General tab rows: (switch action, label, note, on).
    pub(crate) fn general_rows(&self) -> Vec<(Action, &'static str, &'static str, bool)> {
        vec![
            (Action::Startup, "Launch at startup", "Start with Windows", self.startup),
            (Action::AlwaysShow, "Always show", "Stay full size over full-screen and maximised apps", self.prefs.always_show),
        ]
    }


    /// An iOS-style on/off switch (track `sw`×`sh` at x, centred on cy; design pixels).
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn draw_switch(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, x: f32, cy: f32, on: bool, alpha: f32, ox: f32) {
        let s = self.dpi_scale;
        let (sw, sh) = (36.0, 22.0);
        let at = |v: f32| ox + v * s;
        self.brush.SetOpacity(alpha);
        self.brush.SetColor(&rgb(if on { CAMERA_GREEN } else { 0x48484A }, 1.0));
        let track = rect(at(x), (cy - sh / 2.0) * s, at(x + sw), (cy + sh / 2.0) * s);
        t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: track, radiusX: sh / 2.0 * s, radiusY: sh / 2.0 * s }, brush);
        let knob = if on { x + sw - sh / 2.0 } else { x + sh / 2.0 };
        self.brush.SetColor(&rgb(0xFFFFFF, 1.0));
        t.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: at(knob), Y: cy * s }, radiusX: (sh / 2.0 - 2.5) * s, radiusY: (sh / 2.0 - 2.5) * s }, brush);
    }

    /// Settings (replaces the pages while open): show/hide and reorder the pages, then About.
    pub(crate) unsafe fn draw_settings(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
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

        // Header: "Settings" … [Done]
        set(0xFFFFFF, 1.0);
        self.gfx.text(t, "Settings", &self.gfx.fonts.title, rect(at(0.0), y(10.0), at(300.0), y(34.0)), brush);
        let done = Action::CloseSettings;
        let dr = rect(at(w_box - 64.0), y(10.0), at(w_box), y(34.0));
        set(if hovered(&done) { 0x3A3A3C } else { 0x2C2C2E }, 1.0);
        t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: dr, radiusX: y(12.0), radiusY: y(12.0) }, brush);
        set(0xFFFFFF, 1.0);
        self.gfx.text(t, "Done", &self.gfx.fonts.ring_value, dr, brush);
        hits.push((dr, done));
        // Tabs: a segmented control under the title.
        let seg = rect(at(0.0), y(42.0), at(w_box), y(66.0));
        set(0x1C1C1E, 1.0);
        t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: seg, radiusX: y(12.0), radiusY: y(12.0) }, brush);
        let seg_w = w_box / SETTINGS_TABS.len() as f32;
        for (i, name) in SETTINGS_TABS.iter().enumerate() {
            let r = rect(at(i as f32 * seg_w + 2.0), y(44.0), at((i + 1) as f32 * seg_w - 2.0), y(64.0));
            let action = Action::SettingsTab(i);
            if i == self.settings_tab || hovered(&action) {
                set(if i == self.settings_tab { 0x3A3A3C } else { 0x2C2C2E }, 1.0);
                t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(10.0), radiusY: y(10.0) }, brush);
            }
            set(0xFFFFFF, if i == self.settings_tab { 1.0 } else { 0.6 });
            self.gfx.text(t, name, &self.gfx.fonts.ring_value, r, brush);
            if i != self.settings_tab {
                hits.push((r, action));
            }
        }

        match self.settings_tab {
            TAB_GENERAL => {
                for (i, (action, label, note, on)) in self.general_rows().into_iter().enumerate() {
                    let top = SETTINGS_CONTENT + i as f32 * SETTINGS_ROW;
                    self.draw_setting_row(t, brush, top, label, note, hovered(&action), alpha, ox, w_box);
                    self.draw_switch(t, brush, w_box - 48.0, top + (SETTINGS_ROW - 4.0) / 2.0, on, alpha, ox);
                    hits.push((rect(at(0.0), y(top), at(w_box), y(top + SETTINGS_ROW - 4.0)), action));
                }
                return hits;
            }
            TAB_MOTION => {
                // Animation speed: ‹ name › on the right, stepping through SPEEDS.
                let top = SETTINGS_CONTENT;
                self.draw_setting_row(t, brush, top, "Animation speed", "", false, alpha, ox, w_box);
                let (name, _) = settings::SPEEDS[self.prefs.speed];
                let row = |x0: f32, x1: f32| rect(at(x0), y(top + 4.0), at(x1), y(top + SETTINGS_ROW - 8.0));
                set(0x2C2C2E, 1.0);
                t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: row(w_box - 236.0, w_box - 6.0), radiusX: y(10.0), radiusY: y(10.0) }, brush);
                set(0xFFFFFF, 1.0);
                self.gfx.text(t, name, &self.gfx.fonts.ring_value, row(w_box - 206.0, w_box - 36.0), brush);
                for (dir, x0, glyph) in [(-1i32, w_box - 236.0, "‹"), (1, w_box - 36.0, "›")] {
                    let r = row(x0, x0 + 30.0);
                    let next = self.prefs.speed as i32 + dir;
                    let enabled = (0..settings::SPEEDS.len() as i32).contains(&next);
                    let action = Action::Speed(next.max(0) as usize);
                    if enabled && hovered(&action) {
                        set(0x48484A, 1.0);
                        t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(10.0), radiusY: y(10.0) }, brush);
                    }
                    set(0xFFFFFF, if enabled { 1.0 } else { 0.25 });
                    self.gfx.text(t, glyph, &self.gfx.fonts.title, r, brush);
                    if enabled {
                        hits.push((r, action));
                    }
                }

                // Hover delay: a box you type seconds into, "s" after it.
                let top = SETTINGS_CONTENT + SETTINGS_ROW;
                let action = Action::DelayBox;
                self.draw_setting_row(t, brush, top, "Hover delay", "0.15 – 1.5 seconds", false, alpha, ox, w_box);
                let bx = rect(at(w_box - 96.0), y(top + 5.0), at(w_box - 30.0), y(top + SETTINGS_ROW - 9.0));
                let editing = self.delay_edit.is_some();
                set(if editing || hovered(&action) { 0x2C2C2E } else { 0x000000 }, 1.0);
                t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: bx, radiusX: y(8.0), radiusY: y(8.0) }, brush);
                set(if editing { SHELF_BLUE } else { 0x48484A }, 1.0);
                t.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: bx, radiusX: y(8.0), radiusY: y(8.0) }, brush, y(1.5), None);
                let shown = self.delay_edit.clone().unwrap_or_else(|| settings::delay_text(self.prefs.hover_ms));
                let text_x = w_box - 88.0;
                set(0xFFFFFF, 1.0);
                self.gfx.text(t, &shown, &self.gfx.fonts.bubble_text, rect(at(text_x), y(top), at(w_box - 34.0), y(top + SETTINGS_ROW - 4.0)), brush);
                if editing && self.caret_on {
                    let caret_x = text_x + self.gfx.measure(&shown, &self.gfx.fonts.bubble_text) / s + 1.0;
                    set(SHELF_BLUE, 1.0);
                    t.FillRectangle(&rect(at(caret_x), y(top + 10.0), at(caret_x + 1.5), y(top + SETTINGS_ROW - 14.0)), brush);
                }
                set(0x98989F, 1.0);
                self.gfx.text(t, "s", &self.gfx.fonts.bubble_text, rect(at(w_box - 22.0), y(top), at(w_box), y(top + SETTINGS_ROW - 4.0)), brush);
                hits.push((bx, action));

                // Open on: Hover / Click / Both pills.
                let top = SETTINGS_CONTENT + 2.0 * SETTINGS_ROW;
                self.draw_setting_row(t, brush, top, "Open on", "", false, alpha, ox, w_box);
                let pill_w = 62.0;
                let x0 = w_box - 6.0 - pill_w * settings::OPEN_MODES.len() as f32;
                for (k, option) in settings::OPEN_MODES.iter().enumerate() {
                    let r = rect(at(x0 + k as f32 * pill_w + 2.0), y(top + 5.0), at(x0 + (k + 1) as f32 * pill_w - 2.0), y(top + SETTINGS_ROW - 9.0));
                    let action = Action::OpenMode(k);
                    let chosen = k == self.prefs.open_mode;
                    if chosen || hovered(&action) {
                        set(if chosen { 0x48484A } else { 0x2C2C2E }, 1.0);
                        t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(9.0), radiusY: y(9.0) }, brush);
                    }
                    set(0xFFFFFF, if chosen { 1.0 } else { 0.55 });
                    self.gfx.text(t, option, &self.gfx.fonts.ring_value, r, brush);
                    if !chosen {
                        hits.push((r, action));
                    }
                }
                return hits;
            }
            TAB_PAGES => {}
            _ => {
                let top = SETTINGS_CONTENT;
                set(0xFFFFFF, 1.0);
                self.gfx.text(t, "Dynamic Island for Windows", &self.gfx.fonts.pill, rect(at(0.0), y(top), at(w_box), y(top + 22.0)), brush);
                set(0x98989F, 1.0);
                let line = format!("Version {ABOUT_VERSION} · Made by {ABOUT_AUTHOR}");
                self.gfx.text(t, &line, &self.gfx.fonts.time, rect(at(0.0), y(top + 24.0), at(w_box), y(top + 40.0)), brush);
                return hits;
            }
        }
        set(0x6E6E73, 1.0);
        self.gfx.text(t, "DRAG A ROW TO REORDER", &self.gfx.fonts.kind, rect(at(0.0), y(SETTINGS_CONTENT), at(400.0), y(SETTINGS_TOP - 2.0)), brush);

        // Rows: Home (always on, always first), then your order. Each row is drawn at its
        // animated position; the grabbed row last, lifted above the others.
        let rows: Vec<Page> = std::iter::once(Page::Home).chain(self.prefs.order.iter().copied()).collect();
        let switch_x = w_box - 48.0;
        let grabbed_page = self.reorder.and_then(|i| self.prefs.order.get(i).copied());
        let mut draw_order: Vec<usize> = (0..rows.len()).collect();
        draw_order.sort_by_key(|&i| Some(rows[i]) == grabbed_page);
        for i in draw_order {
            let page = &rows[i];
            let slot = SETTINGS_TOP + i as f32 * SETTINGS_ROW;
            let top = self.row_y.iter().find(|(p, _)| p == page).map_or(slot, |(_, sp)| sp.value);
            let mid = top + (SETTINGS_ROW - 4.0) / 2.0;
            let r = rect(at(0.0), y(top), at(w_box), y(top + SETTINGS_ROW - 4.0));
            let order_index = i.checked_sub(1);
            let grabbed = order_index.is_some() && self.reorder == order_index;
            let grip = order_index.map(Action::TabGrip);
            let hot = grip.as_ref().is_some_and(&hovered);
            let r = if grabbed {
                // Lifted: a soft shadow under it, and a touch bigger.
                for k in 1..=4 {
                    let spread = k as f32 * 2.0;
                    set(0x000000, 0.12);
                    let shade = rect(at(-spread / 2.0), y(top + 3.0 - spread / 2.0), at(w_box + spread / 2.0), y(top + SETTINGS_ROW - 1.0 + spread / 2.0));
                    t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: shade, radiusX: y(10.0 + spread / 2.0), radiusY: y(10.0 + spread / 2.0) }, brush);
                }
                rect(at(-3.0), y(top - 1.5), at(w_box + 3.0), y(top + SETTINGS_ROW - 2.5))
            } else {
                r
            };
            set(if grabbed { 0x3A3A3C } else if hot { 0x2C2C2E } else { 0x1C1C1E }, 1.0);
            t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: y(10.0), radiusY: y(10.0) }, brush);
            let on = !self.prefs.hidden.contains(page);
            if let Some(grip) = grip {
                set(if hot || grabbed { 0xFFFFFF } else { 0x6E6E73 }, 1.0);
                self.gfx.icon(t, Icon::Grip, at(16.0), y(mid), y(16.0), brush);
                hits.push((rect(at(0.0), y(top), at(switch_x - 8.0), y(top + SETTINGS_ROW - 4.0)), grip));
            }
            set(0xFFFFFF, if on { 1.0 } else { 0.45 });
            let name = settings::name(*page);
            self.gfx.text(t, name, &self.gfx.fonts.pill, rect(at(34.0), y(top), at(260.0), y(top + SETTINGS_ROW - 4.0)), brush);
            let note = match page {
                Page::Home => "Always first",
                Page::Music => "Shows while something is playing",
                Page::Shelf => "Shows when it holds files",
                _ => "",
            };
            let name_w = self.gfx.measure(name, &self.gfx.fonts.pill) / s;
            set(0x6E6E73, 1.0);
            self.gfx.text(t, note, &self.gfx.fonts.time, rect(at(34.0 + name_w + 10.0), y(top), at(switch_x - 10.0), y(top + SETTINGS_ROW - 4.0)), brush);
            self.draw_switch(t, brush, switch_x, mid, on, alpha * if *page == Page::Home { 0.4 } else { 1.0 }, ox);
            if *page != Page::Home {
                hits.insert(0, (rect(at(switch_x - 6.0), y(top), at(w_box), y(top + SETTINGS_ROW - 4.0)), Action::TabToggle(*page)));
            }
        }

        hits
    }

    /// A Settings row's background, label and grey note (the control on the right is drawn
    /// by the caller).
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn draw_setting_row(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, top: f32, label: &str, note: &str, hot: bool, alpha: f32, ox: f32, w_box: f32) {
        let s = self.dpi_scale;
        let at = |x: f32| ox + x * s;
        let set = |hex: u32| {
            self.brush.SetColor(&rgb(hex, 1.0));
            self.brush.SetOpacity(alpha);
        };
        set(if hot { 0x2C2C2E } else { 0x1C1C1E });
        let r = rect(at(0.0), top * s, at(w_box), (top + SETTINGS_ROW - 4.0) * s);
        t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: 10.0 * s, radiusY: 10.0 * s }, brush);
        set(0xFFFFFF);
        let text_r = |x0: f32, x1: f32| rect(at(x0), top * s, at(x1), (top + SETTINGS_ROW - 4.0) * s);
        self.gfx.text(t, label, &self.gfx.fonts.pill, text_r(14.0, 300.0), brush);
        let label_w = self.gfx.measure(label, &self.gfx.fonts.pill) / s;
        set(0x6E6E73);
        self.gfx.text(t, note, &self.gfx.fonts.time, text_r(14.0 + label_w + 10.0, w_box - 60.0), brush);
    }
}
