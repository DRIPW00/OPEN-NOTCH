//! The system tray icon (the "show hidden icons" area by the clock).
//! Click it (left or right) for a menu with "Quit Dynamic Island".

use std::cell::Cell;

use windows::core::{w, Result};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
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
        let class_name = w!("DynamicIslandTray");
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
            w!("Dynamic Island tray"),
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
        ICON.with(|icon| icon.set(draw_icon().unwrap_or_else(|_| fallback_icon())));
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
    let tip: Vec<u16> = "Dynamic Island".encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    data
}

unsafe fn add_icon(hwnd: HWND) {
    let data = icon_data(hwnd);
    let _ = Shell_NotifyIconW(NIM_ADD, &data);
}

unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, w!("Dynamic Island"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit Dynamic Island"));

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

/// Draw a tiny notch for the tray: a black pill with a white outline, so it's visible on
/// both dark and light taskbars. Drawn at the size Windows wants for small icons.
unsafe fn draw_icon() -> Result<HICON> {
    let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()).max(16);

    // A 32-bit bitmap with an alpha channel to draw into.
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = CreateCompatibleDC(None);
    let mut bits = std::ptr::null_mut();
    let color = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
    let previous = SelectObject(dc, color.into());

    let result = (|| -> Result<()> {
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let target = factory.CreateDCRenderTarget(&props)?;
        target.BindDC(dc, &RECT { left: 0, top: 0, right: size, bottom: size })?;

        let s = size as f32;
        let (w, h) = (s * 0.92, s * 0.46);
        let border = (s / 16.0).max(1.0);
        let pill = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: (s - w) / 2.0,
                top: (s - h) / 2.0,
                right: (s + w) / 2.0,
                bottom: (s + h) / 2.0,
            },
            radiusX: h / 2.0,
            radiusY: h / 2.0,
        };
        let black = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }, None)?;
        let white = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;

        target.BeginDraw();
        target.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        target.FillRoundedRectangle(&pill, &black);
        let mut inner = pill;
        inner.rect.left += border / 2.0;
        inner.rect.top += border / 2.0;
        inner.rect.right -= border / 2.0;
        inner.rect.bottom -= border / 2.0;
        inner.radiusX -= border / 2.0;
        inner.radiusY -= border / 2.0;
        target.DrawRoundedRectangle(&inner, &white, border, None);
        target.EndDraw(None, None)
    })();

    SelectObject(dc, previous);
    let _ = DeleteDC(dc);
    if let Err(error) = result {
        let _ = DeleteObject(color.into());
        return Err(error);
    }

    // Direct2D wrote premultiplied colour; icons expect straight alpha. Convert so the
    // anti-aliased edges aren't darkened.
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
    for px in pixels.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }

    // Icons also need a 1-bit mask; it's ignored when the colour bitmap has alpha.
    let mask = CreateBitmap(size, size, 1, 1, None);
    let icon_info = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&icon_info);
    let _ = DeleteObject(mask.into());
    let _ = DeleteObject(color.into());
    icon
}

/// Windows' generic app icon, in case drawing our own ever fails.
unsafe fn fallback_icon() -> HICON {
    LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
}
