//! File shelf: drop files on the notch to keep them, drag them back out to use them.
//!
//! - Drop target: the notch window accepts files dragged from Explorer (or any app that
//!   drags files). The shelf keeps a reference to each file (its path); nothing is copied.
//! - Drag out: hands the file to Windows' own drag and drop, so it works like dragging
//!   from Explorer (drop on a folder, the desktop, a chat, an upload box…), but always as
//!   a copy: the original stays where it is.
//! - Thumbnails: Windows' own previews (photos show the picture, other files their icon),
//!   made on a background thread.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use windows::core::{implement, Ref, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted when a thumbnail is ready.
pub const WM_SHELF_THUMB: u32 = WM_APP + 12;
/// Thumbnail size in pixels (square).
pub const THUMB: u32 = 96;

/// What happened during a drag over the notch.
pub enum DropEvent {
    Enter,
    Leave,
    Drop(Vec<String>),
}

/// The notch's drop target. Windows calls it (on the window's thread) while files are
/// dragged over the notch; `on_event` passes that on and says whether we'd accept.
#[implement(IDropTarget)]
pub struct DropTarget {
    on_event: Box<dyn Fn(DropEvent) -> bool>,
    accept: std::cell::Cell<bool>,
}

impl DropTarget {
    pub fn new(on_event: impl Fn(DropEvent) -> bool + 'static) -> DropTarget {
        DropTarget { on_event: Box::new(on_event), accept: std::cell::Cell::new(false) }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        // Only file drags (not text or links).
        let has_files = data.as_ref().is_some_and(|d| unsafe { d.QueryGetData(&hdrop_format()).is_ok() });
        let accept = has_files && (self.on_event)(DropEvent::Enter);
        self.accept.set(accept);
        unsafe { *effect = if accept { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        unsafe { *effect = if self.accept.get() { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if self.accept.replace(false) {
            (self.on_event)(DropEvent::Leave);
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = data.as_ref().map(|d| unsafe { files(d) }).unwrap_or_default();
        let accepted = self.accept.replace(false) && !files.is_empty();
        if accepted {
            (self.on_event)(DropEvent::Drop(files));
        } else {
            (self.on_event)(DropEvent::Leave);
        }
        unsafe { *effect = if accepted { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

/// The file paths in a drag.
unsafe fn files(data: &IDataObject) -> Vec<String> {
    let Ok(mut medium) = data.GetData(&hdrop_format()) else { return Vec::new() };
    let drop = HDROP(medium.u.hGlobal.0);
    let count = DragQueryFileW(drop, u32::MAX, None);
    let mut out = Vec::new();
    for i in 0..count {
        let len = DragQueryFileW(drop, i, None) as usize;
        let mut buffer = vec![0u16; len + 1];
        DragQueryFileW(drop, i, Some(&mut buffer));
        out.push(String::from_utf16_lossy(&buffer[..len]));
    }
    ReleaseStgMedium(&mut medium);
    out
}

/// Let the notch window accept dropped files. Needs OleInitialize on this thread first.
pub unsafe fn register(hwnd: HWND, target: DropTarget) -> bool {
    let target: IDropTarget = target.into();
    RegisterDragDrop(hwnd, &target).is_ok()
}

/// Drag these files out of the notch with Windows' drag and drop (its usual drag picture
/// and cursor included). Returns once the drop is done or cancelled.
pub unsafe fn drag_out(hwnd: HWND, paths: &[String]) {
    let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
    for path in paths {
        let mut pidl = std::ptr::null_mut();
        if SHParseDisplayName(&HSTRING::from(path.as_str()), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
            pidls.push(pidl);
        }
    }
    if !pidls.is_empty() {
        let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        if let Ok(items) = SHCreateShellItemArrayFromIDLists(&list) {
            if let Ok(data) = items.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) {
                // Copy only: dropping somewhere never moves the original file.
                let _ = SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY);
            }
        }
    }
    for pidl in pidls {
        ILFree(Some(pidl as *const _));
    }
}

/// Makes thumbnails on a background thread; results are collected here.
pub struct Thumbs {
    ready: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    requests: Sender<String>,
}

impl Thumbs {
    pub fn start(notify: HWND) -> Thumbs {
        let ready = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = channel::<String>();
        let out = ready.clone();
        let notify = notify.0 as isize;
        let _ = std::thread::Builder::new().name("shelf-thumbs".into()).spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            while let Ok(path) = rx.recv() {
                let pixels = thumbnail(&path).unwrap_or_default();
                if let Ok(mut r) = out.lock() {
                    r.insert(path, pixels);
                }
                let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_SHELF_THUMB, WPARAM(0), LPARAM(0));
            }
        });
        Thumbs { ready, requests: tx }
    }

    pub fn request(&self, path: &str) {
        let _ = self.requests.send(path.to_string());
    }

    /// Finished thumbnails (THUMB × THUMB premultiplied BGRA; empty if none could be made).
    pub fn take(&self) -> Vec<(String, Vec<u8>)> {
        self.ready.lock().map(|mut r| r.drain().collect()).unwrap_or_default()
    }
}

/// Windows' preview of a file as THUMB×THUMB premultiplied BGRA pixels. Photos fill the
/// square (cropped to the middle); icons keep their shape with some room around them.
unsafe fn thumbnail(path: &str) -> Option<Vec<u8>> {
    let item: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
    let bitmap = item.GetImage(SIZE { cx: THUMB as i32, cy: THUMB as i32 }, SIIGBF_BIGGERSIZEOK).ok()?;
    let pixels = read_bitmap(bitmap);
    let _ = DeleteObject(bitmap.into());
    let (w, h, mut px) = pixels?;

    // Any see-through pixels? Then it's an icon (keeps its shape); else a photo (fill).
    // Some previews come without alpha at all (all 0): those are photos too.
    let no_alpha = px.chunks(4).all(|p| p[3] == 0);
    let icon = !no_alpha && px.chunks(4).any(|p| p[3] < 250);
    if !icon {
        for p in px.chunks_mut(4) {
            p[3] = 255;
        }
    }
    let n = THUMB as usize;
    let mut out = vec![0u8; n * n * 4];
    // Scale: photos cover the square, icons fit inside 80% of it.
    let scale = if icon { (n as f32 * 0.8) / w.max(h) as f32 } else { n as f32 / w.min(h) as f32 };
    let (dw, dh) = (w as f32 * scale, h as f32 * scale);
    let (ox, oy) = ((n as f32 - dw) / 2.0, (n as f32 - dh) / 2.0);
    for y in 0..n {
        for x in 0..n {
            let sx = ((x as f32 - ox) / scale).floor();
            let sy = ((y as f32 - oy) / scale).floor();
            if sx < 0.0 || sy < 0.0 || sx >= w as f32 || sy >= h as f32 {
                continue;
            }
            let si = (sy as usize * w + sx as usize) * 4;
            let di = (y * n + x) * 4;
            let a = px[si + 3] as u32;
            // Premultiply for Direct2D.
            out[di] = (px[si] as u32 * a / 255) as u8;
            out[di + 1] = (px[si + 1] as u32 * a / 255) as u8;
            out[di + 2] = (px[si + 2] as u32 * a / 255) as u8;
            out[di + 3] = a as u8;
        }
    }
    Some(out)
}

/// A bitmap's pixels as top-down 32-bit BGRA: (width, height, pixels).
unsafe fn read_bitmap(bitmap: HBITMAP) -> Option<(usize, usize, Vec<u8>)> {
    let mut info = BITMAP::default();
    if GetObjectW(bitmap.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut _)) == 0 {
        return None;
    }
    let (w, h) = (info.bmWidth.max(1) as usize, info.bmHeight.unsigned_abs().max(1) as usize);
    let mut header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut px = vec![0u8; w * h * 4];
    let dc = GetDC(None);
    let rows = GetDIBits(dc, bitmap, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut header, DIB_RGB_COLORS);
    ReleaseDC(None, dc);
    (rows > 0).then_some((w, h, px))
}

/// Text on the clipboard (for pasting a Wi-Fi password).
pub unsafe fn clipboard_text(hwnd: HWND) -> Option<String> {
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    OpenClipboard(Some(hwnd)).ok()?;
    let text = GetClipboardData(CF_UNICODETEXT.0 as u32).ok().and_then(|handle| {
        let global = HGLOBAL(handle.0);
        let ptr = GlobalLock(global) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let text = PCWSTR(ptr).to_string().ok();
        let _ = GlobalUnlock(global);
        text
    });
    let _ = CloseClipboard();
    text
}

/// "Invoice_Nov.pdf" from a full path.
pub fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}
