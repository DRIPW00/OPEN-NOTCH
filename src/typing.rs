//! Typing in the notch: the Wi-Fi password box, the hover-delay box and keyboard focus.

use super::*;

impl App {

    pub(crate) unsafe fn open_password(&mut self, ssid: &str) {
        self.pw_ssid = Some(ssid.to_string());
        self.pw_text.clear();
        self.pw_show = false;
        self.focus_password();
    }

    /// Give the password field the keyboard. The notch normally never takes focus, so it
    /// becomes an ordinary active window just while you type.
    pub(crate) unsafe fn focus_password(&mut self) {
        if self.pw_ssid.is_none() {
            return;
        }
        self.pw_focus = true;
        self.take_keyboard();
        self.redraw();
    }

    /// The notch normally never takes focus; it becomes an ordinary active window while you
    /// type in one of its boxes (Wi-Fi password, hover delay), with a blinking caret.
    pub(crate) unsafe fn take_keyboard(&mut self) {
        let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, style & !(WS_EX_NOACTIVATE.0 as isize));
        let _ = SetForegroundWindow(self.hwnd);
        let _ = SetFocus(Some(self.hwnd));
        self.caret_on = true;
        SetTimer(Some(self.hwnd), CARET_TIMER, 530, None);
    }

    /// Typing in a box: stay open, and keys go to it.
    pub(crate) fn typing(&self) -> bool {
        self.pw_focus || self.delay_edit.is_some()
    }

    /// Back to never taking focus. If the pointer is already away, close like a normal leave.
    pub(crate) unsafe fn release_keyboard(&mut self) {
        let _ = KillTimer(Some(self.hwnd), CARET_TIMER);
        let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as isize);
        if !self.tracking && !self.pinned && self.hovered {
            self.leaving = true;
            SetTimer(Some(self.hwnd), LEAVE_TIMER, LEAVE_MS, None);
        }
        self.redraw();
    }

    /// The field lost the keyboard (clicked elsewhere, Esc, joined).
    pub(crate) unsafe fn blur_password(&mut self) {
        if !self.pw_focus {
            return;
        }
        self.pw_focus = false;
        self.release_keyboard();
    }

    /// Done typing a hover delay (Enter, clicked elsewhere): keep it if it's a number,
    /// pulled into 0.15–1.5 s. `keep: false` (Esc) leaves the old value.
    pub(crate) unsafe fn end_delay_edit(&mut self, keep: bool) {
        let Some(text) = self.delay_edit.take() else { return };
        if let Some(ms) = settings::parse_delay(&text).filter(|_| keep) {
            self.prefs.hover_ms = ms;
            self.prefs.save();
        }
        self.release_keyboard();
    }

    pub(crate) unsafe fn close_password(&mut self) {
        self.blur_password();
        self.pw_ssid = None;
        self.pw_text.clear(); // don't keep the password around
    }

    pub(crate) unsafe fn submit_password(&mut self) {
        let Some(ssid) = self.pw_ssid.clone() else { return };
        if self.pw_text.chars().count() < 8 || self.wifi_state.joining.is_some() {
            return;
        }
        if let Some(network) = self.wifi_state.networks.iter().find(|n| n.ssid == ssid).cloned() {
            self.wifi.join(&network, &self.pw_text);
            self.wifi_state = self.wifi.state();
        }
        self.blur_password();
    }

    /// A key typed into the password field or the hover delay box.
    pub(crate) unsafe fn on_char(&mut self, ch: u32) {
        if let Some(text) = &mut self.delay_edit {
            match ch {
                0x08 => {
                    text.pop();
                }
                0x0D => return self.end_delay_edit(true),
                0x1B => return self.end_delay_edit(false),
                c if (c == u32::from(b'.') || char::from_u32(c).is_some_and(|c| c.is_ascii_digit())) && text.len() < 5 => {
                    text.push(c as u8 as char);
                }
                _ => {}
            }
            self.caret_on = true;
            return self.redraw();
        }
        if !self.pw_focus {
            return;
        }
        match ch {
            0x08 => {
                self.pw_text.pop();
            }
            0x0D => return self.submit_password(),
            0x1B => {
                self.close_password();
                return self.set_expanded(self.hovered);
            }
            0x16 => {
                // Ctrl+V
                if let Some(text) = shelf::clipboard_text(self.hwnd) {
                    self.pw_text.extend(text.chars().filter(|c| !c.is_control()));
                }
            }
            c if c >= 0x20 => {
                if let Some(c) = char::from_u32(c) {
                    self.pw_text.push(c);
                }
            }
            _ => {}
        }
        self.pw_text = self.pw_text.chars().take(63).collect(); // Wi-Fi passwords are at most 63
        if let Some((ssid, _)) = &self.wifi_state.error {
            if self.pw_ssid.as_ref() == Some(ssid) {
                self.wifi_state.error = None; // typing again clears the old error
            }
        }
        self.caret_on = true;
        self.redraw();
    }

}
