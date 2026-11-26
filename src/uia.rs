//! Shuffle and repeat for apps that don't expose them to Windows' media controls.
//!
//! Browsers (YouTube Music in Opera/Chrome/Edge...) and Apple Music can't receive shuffle
//! or repeat commands through the media controls: the web standard has no such actions,
//! and Apple Music doesn't offer them. So we press the app's OWN button instead, through
//! UI Automation (the accessibility system screen readers use). This doesn't move the
//! mouse, switch windows or type anything.
//!
//! Finding the right button: pages can have several "Shuffle" buttons (e.g. a playlist's
//! "Shuffle" next to the player's). We pick the one closest to the player's play/pause
//! button, which is always part of the player controls.

use windows::core::BOOL;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::media::Repeat;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Shuffle,
    Repeat,
}

/// What we could read from the button after pressing it.
pub struct Pressed {
    pub shuffle: Option<bool>,
    pub repeat: Option<Repeat>,
}

/// Press the app's shuffle/repeat button. `None` if no suitable button was found.
pub unsafe fn press(app_name: &str, title: &str, playing: bool, kind: Kind) -> Option<Pressed> {
    let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
    let buttons = uia
        .CreatePropertyCondition(UIA_ControlTypePropertyId, &variant_i32(UIA_ButtonControlTypeId.0))
        .ok()?;

    for hwnd in app_windows(app_name, title) {
        let Ok(root) = uia.ElementFromHandle(hwnd) else { continue };
        // Chromium-based browsers build the page's accessibility tree on first request, so
        // the first search may only see the browser's own buttons: if so, look again.
        let mut button = pick(&collect(&root, &buttons), kind, playing);
        if button.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(600));
            button = pick(&collect(&root, &buttons), kind, playing);
        }
        let Some(button) = button else { continue };

        let invoked = button
            .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            .and_then(|p| p.Invoke())
            .is_ok()
            || button
                .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                .and_then(|p| p.Toggle())
                .is_ok();
        if !invoked {
            continue;
        }

        // Let the app update the button, then read its new state.
        std::thread::sleep(std::time::Duration::from_millis(250));
        let name = button.CurrentName().map(|n| n.to_string().to_lowercase()).unwrap_or_default();
        let toggled = button
            .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            .and_then(|p| p.CurrentToggleState())
            .ok()
            .map(|s| s == ToggleState_On);
        return Some(match kind {
            Kind::Shuffle => Pressed { shuffle: toggled, repeat: None },
            Kind::Repeat => {
                let mode = if name.contains("one") || name.contains("track") || name.contains("song") {
                    Some(Repeat::One)
                } else if name.contains("all") || name.contains("playlist") || name.contains("queue") {
                    Some(Repeat::All)
                } else if name.contains("off") {
                    Some(Repeat::Off)
                } else {
                    toggled.map(|on| if on { Repeat::All } else { Repeat::Off })
                };
                Pressed { shuffle: None, repeat: mode }
            }
        });
    }
    None
}

/// A VARIANT holding a 32-bit integer (what UI Automation compares control types with).
fn variant_i32(value: i32) -> VARIANT {
    let mut v = VARIANT::default();
    // SAFETY: setting the type tag and the matching union field together.
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = value;
    }
    v
}

struct Button {
    element: IUIAutomationElement,
    name: String,
    centre: (f32, f32),
}

unsafe fn collect(root: &IUIAutomationElement, condition: &IUIAutomationCondition) -> Vec<Button> {
    let Ok(all) = root.FindAll(TreeScope_Descendants, condition) else { return Vec::new() };
    let count = all.Length().unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..count {
        let Ok(element) = all.GetElement(i) else { continue };
        let name = element.CurrentName().map(|n| n.to_string().to_lowercase()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let r = element.CurrentBoundingRectangle().unwrap_or_default();
        if r.right <= r.left {
            continue; // off-screen / hidden
        }
        let centre = ((r.left + r.right) as f32 / 2.0, (r.top + r.bottom) as f32 / 2.0);
        out.push(Button { element, name, centre });
    }
    out
}

fn pick(buttons: &[Button], kind: Kind, playing: bool) -> Option<IUIAutomationElement> {
    let wanted = |name: &str| match kind {
        Kind::Shuffle => name.starts_with("shuffle"),
        Kind::Repeat => name.starts_with("repeat") || name.starts_with("loop"),
    };
    // Exclude "Shuffle play"-style buttons that start playback rather than toggle shuffle.
    let candidates: Vec<&Button> = buttons.iter().filter(|b| wanted(&b.name) && !b.name.contains("play")).collect();
    if candidates.is_empty() {
        return None;
    }
    // The player's play/pause button: "Pause" while playing, else "Play".
    let anchor_name = if playing { "pause" } else { "play" };
    let anchor = buttons.iter().find(|b| b.name == anchor_name || b.name.starts_with(&format!("{anchor_name} ")));
    let best = match anchor {
        Some(a) => candidates.into_iter().min_by(|x, y| {
            let d = |b: &Button| (b.centre.0 - a.centre.0).powi(2) + (b.centre.1 - a.centre.1).powi(2);
            d(x).total_cmp(&d(y))
        }),
        None => candidates.into_iter().next(),
    };
    best.map(|b| b.element.clone())
}

/// Visible top-level windows of the app behind a session, the one showing the song first.
unsafe fn app_windows(app_name: &str, title: &str) -> Vec<HWND> {
    let exes: Vec<String> = match app_name {
        "Apple Music" => vec!["applemusic.exe".into()],
        "iTunes" => vec!["itunes.exe".into()],
        "Opera" => vec!["opera.exe".into(), "opera_gx.exe".into()],
        "Chrome" => vec!["chrome.exe".into()],
        "Edge" => vec!["msedge.exe".into()],
        other => vec![format!("{}.exe", other.to_lowercase().replace(' ', ""))],
    };
    struct Search {
        exes: Vec<String>,
        found: Vec<(HWND, String)>,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        let search = &mut *(data.0 as *mut Search);
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if let Some(exe) = process_exe(pid) {
            if search.exes.contains(&exe) {
                let mut text = [0u16; 512];
                let len = GetWindowTextW(hwnd, &mut text) as usize;
                let title = String::from_utf16_lossy(&text[..len]).to_lowercase();
                if !title.is_empty() {
                    search.found.push((hwnd, title));
                }
            }
        }
        true.into()
    }
    let mut search = Search { exes, found: Vec::new() };
    let _ = EnumWindows(Some(each), LPARAM(&mut search as *mut Search as isize));

    // Windows whose title mentions the song (a browser tab's title) come first.
    let title = title.to_lowercase();
    search.found.sort_by_key(|(_, t)| !(t.contains(&title) || t.contains("music")));
    search.found.into_iter().map(|(h, _)| h).collect()
}

/// Lower-case file name of a process's executable, e.g. "opera.exe".
unsafe fn process_exe(pid: u32) -> Option<String> {
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut len);
    let _ = CloseHandle(process);
    ok.ok()?;
    let path = String::from_utf16_lossy(&buffer[..len as usize]);
    path.rsplit('\\').next().map(|name| name.to_lowercase())
}
