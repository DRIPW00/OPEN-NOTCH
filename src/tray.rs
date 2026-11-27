//! The system tray icon (the "show hidden icons" area by the clock).
//! Click it (left or right) for a menu with "Quit OpenNotch".

use std::cell::Cell;

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Message the tray icon sends us when it's clicked.
const WM_TRAY_ICON: u32 = WM_APP + 1;
const ICON_ID: u32 = 1;
const MENU_QUIT: usize = 100;

thread_local! {
    static ICON: Cell<HICON> = const { Cell::new(HICON(std::ptr::null_mut())) };
    /// Sent by Windows when Explorer (and the taskbar) restarts; we re-add our icon then.
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

pub struct Tray {
    hwnd: HWND,
}

impl Tray {
    pub unsafe fn create(instance: HINSTANCE) -> Result<Tray> {
        let class_name = w!("OpenNotchTray");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(tray_wnd_proc),
            hInstance: instance,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wc);

        // A hidden window that receives the tray icon's clicks. It's never shown.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name,
            w!("OpenNotch tray"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )?;

        TASKBAR_CREATED.with(|m| m.set(RegisterWindowMessageW(w!("TaskbarCreated"))));
        ICON.with(|icon| icon.set(load_icon(instance)));
        add_icon(hwnd);
        Ok(Tray { hwnd })
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let data = icon_data(self.hwnd);
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            let icon = ICON.with(|icon| icon.replace(HICON(std::ptr::null_mut())));
            if !icon.is_invalid() {
                let _ = DestroyIcon(icon);
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY_ICON,
        hIcon: ICON.with(|icon| icon.get()),
        ..Default::default()
    };
    let tip: Vec<u16> = "OpenNotch".encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data
}

unsafe fn add_icon(hwnd: HWND) {
    let data = icon_data(hwnd);
    let _ = Shell_NotifyIconW(NIM_ADD, &data);
}

unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, w!("OpenNotch"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit OpenNotch"));

    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    // Required so the menu closes when you click somewhere else.
    let _ = SetForegroundWindow(hwnd);
    let choice = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY | TPM_BOTTOMALIGN,
        cursor.x,
        cursor.y,
        None,
        hwnd,
        None,
    );
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);

    if choice.0 as usize == MENU_QUIT {
        PostQuitMessage(0);
    }
}

extern "system" fn tray_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_TRAY_ICON {
            let event = (lparam.0 & 0xFFFF) as u32;
            if event == WM_LBUTTONUP || event == WM_RBUTTONUP {
                show_menu(hwnd);
            }
            return LRESULT(0);
        }
        if msg != 0 && msg == TASKBAR_CREATED.with(|m| m.get()) {
            add_icon(hwnd);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// The app's icon (resource 1, see build.rs) at the size Windows wants for the tray; the
/// small sizes in it are drawn for the tray. Windows' default app icon if it's missing.
unsafe fn load_icon(instance: HINSTANCE) -> HICON {
    let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()).max(16);
    match LoadImageW(Some(instance), PCWSTR(std::ptr::without_provenance(1)) /* MAKEINTRESOURCE(1) */, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
        Ok(handle) => HICON(handle.0),
        Err(_) => LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
    }
}
