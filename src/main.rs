// Dynamic Island for Windows.
//
// How it works:
// - One borderless, always-on-top "layered" window sits at the top centre of the screen,
//   drawn as a MacBook-style notch flush with the top edge.
// - We draw with Direct2D (anti-aliased) into an off-screen bitmap, then hand that bitmap to
//   Windows with UpdateLayeredWindow. Fully transparent pixels let mouse clicks pass straight
//   through to whatever is underneath.
// - Idle (nothing playing): the time and a battery ring, plus a green camera / orange mic
//   dot while an app uses them. Hovered: big time, the day (red) and date, the battery %.
// - Now Playing: media.rs reads every app's media session (Apple Music, Spotify, browsers...).
//   Closed, the notch shows the album art and a live waveform; hovered, it opens into a card
//   with source tabs, the song, a seek bar and controls.
// - While the notch moves we draw one frame per refresh of its monitor (60/120/144/240 Hz...).
//   While music plays we redraw the waveform at 30 fps (closed) or the progress bar at 4 fps
//   (open). Otherwise nothing is drawn and the app just waits (~0% CPU).
// - Stays above other always-on-top windows (re-asserted when the active app changes).
// - Controls (a third page): volume and brightness sliders, the audio output and input
//   devices, and mute. Pressing a volume or brightness key briefly shows a slider in the
//   closed notch.
// - Network (Wi-Fi networks, Bluetooth devices) and System (CPU, RAM, GPU, SSD) pages.
// - File shelf: drop files on the notch to keep them; drag them back out from the Shelf page.
// - A pin (top-right of the open card) keeps it open. While an app is full screen or
//   maximised the notch shrinks to a thin bar; resting the pointer on it for 0.75 s opens it.
// - Settings (gear on Home): show/hide and reorder the pages; saved between runs.
// - Quit from the tray icon by the clock. (Clicking the notch never closes it.)

// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod bluetooth;
mod brightness;
mod gfx;
mod media;
mod privacy;
mod radios;
mod shelf;
mod spring;
mod startup;
mod stats;
mod settings;
mod sysinfo;
mod tray;
mod wifi;

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gfx::{rect, rgb, Gfx, Icon};
use media::{Command, Media, MediaSnapshot, Repeat, Session, Status, WM_MEDIA_UPDATE};
use spring::{Ease, Spring, COLLAPSE, EXPAND, POP_IN, POP_OUT, ROW};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};

// ---- Layout, in logical pixels (scaled by the monitor's DPI) ----
const CANVAS_W: f32 = 700.0; // window size: big enough for the open card + spring overshoot
const CANVAS_H: f32 = 420.0; // tall enough for the Controls page with a device list open

// Notch sizes (width, height). Flush with the top edge of the screen.
const CLOSED: (f32, f32) = (270.0, 32.0); // closed: always the Home view (time, day, battery)
/// The open card: one width for every page, so switching pages only slides the content.
const CARD_W: f32 = 604.0;
const HOME_H: f32 = 112.0; // Home page (clock card, design 6A) incl. the page dots
const MUSIC_H: f32 = 198.0; // Music page incl. the page dots
const CONTROLS_H: f32 = 160.0; // Controls page incl. the page dots (device list adds more)
/// Closed notch while the volume / brightness slider shows (after a key press).
const HUD: (f32, f32) = (290.0, 36.0);
/// The thin bar the notch shrinks to while an app is full screen or maximised.
const BAR: (f32, f32) = (180.0, 6.0);
/// Most devices listed in a picker (keeps the card on screen).
const MAX_DEVICES: usize = 6;
const NET_H: f32 = 100.0; // Network page incl. the page dots (a list adds more)
const SYSTEM_H: f32 = 128.0; // System page incl. the page dots
const SHELF_H: f32 = 136.0; // Shelf page incl. the page dots
/// Settings: header, one row per page, then About.
const SETTINGS_ROW: f32 = 36.0;
const SETTINGS_TOP: f32 = 92.0; // first page row (Pages tab)
const SETTINGS_CONTENT: f32 = 76.0; // first row of the other tabs
/// Settings is split into these tabs (a segmented control under the title).
const SETTINGS_TABS: [&str; 4] = ["General", "Motion", "Pages", "About"];
const TAB_GENERAL: usize = 0;
const TAB_MOTION: usize = 1;
const TAB_PAGES: usize = 2;
/// Shown under About in Settings.
const ABOUT_VERSION: &str = "R1.02";
const ABOUT_AUTHOR: &str = "DRIPWOO";
/// The notch while files are dragged over it (the drop zone).
const DROP: (f32, f32) = (CARD_W, 140.0);
/// Extra closed width while the shelf holds files (room for their mini thumbnails).
const SHELF_CLOSED_EXTRA: f32 = 54.0;
/// Shelf tiles visible at once (the last slot is the drop hint).
const SHELF_TILES: usize = 5;
/// The password row under a Wi-Fi network being joined.
const PASSWORD_ROW: f32 = 46.0;
const SHELF_BLUE: u32 = 0x0A84FF;
/// Header row (title + refresh) above a Wi-Fi / Bluetooth list.
const LIST_HEADER: f32 = 30.0;
/// The shortest page. How "open" the notch is (content fade-in, clicks enabled) is measured
/// against it, so every page counts as fully open once the notch reaches its own height.
const SHORTEST_PAGE_H: f32 = if NET_H < HOME_H { NET_H } else { HOME_H };
/// Music page: margin from the card edge to its content (side strip + 20px).
const MUSIC_PAD: f32 = 32.0;
/// Home page layout box inside the card (centred).
const OPEN_IDLE: (f32, f32) = (580.0, 104.0);
/// Click strips at the card's left/right edges that switch pages.
const SIDE: f32 = 24.0;
const SHOULDER: f32 = 8.0; // outward curve where the notch meets the top edge
const MIN_CORNER: f32 = 10.0; // bottom corner radius when closed...
const MAX_CORNER: f32 = 26.0; // ...and when open
const BEZIER_CIRCLE: f32 = 0.552_284_8; // makes cubic curves trace a true quarter-circle

// Defined in the large Win32_UI_Controls module; declared here to keep the build lean.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Redraw timer for the waveform / progress bar while music plays.
const TICK_TIMER: usize = 1;
/// Clears a short notice in the card.
const NOTICE_TIMER: usize = 2;
/// Backup check that keeps the notch above other always-on-top windows.
const TOPMOST_TIMER: usize = 3;
const NOTICE_MS: u32 = 3000;
/// Fires at the start of every minute to update the clock (and battery).
const CLOCK_TIMER: usize = 4;
/// WM_POWERBROADCAST: battery level or charger changed.
const PBT_APMPOWERSTATUSCHANGE: usize = 0xA;

// Colours
const CAMERA_GREEN: u32 = 0x30D158;
const MIC_ORANGE: u32 = 0xFF9F0A;
const DAY_RED: u32 = 0xFF453A;
const BRIGHT_YELLOW: u32 = 0xFFD60A;

/// Hides the volume / brightness slider in the closed notch after a key press.
const HUD_TIMER: usize = 6;
const HUD_MS: u32 = 1600;
/// When the pointer leaves the open notch, wait this long before closing (it may come back).
const LEAVE_TIMER: usize = 7;
const LEAVE_MS: u32 = 450;
/// Hover delay (Settings > Motion): opens the card if the pointer is still there.
const HOVER_TIMER: usize = 10;
/// In bar mode (full-screen app) the bar lets clicks through, so we can't see the pointer
/// arrive; instead we check where it is every 100 ms. Resting on the bar for 0.75 s opens it.
const BAR_POLL_TIMER: usize = 8;
const BAR_POLL_MS: u32 = 100;
/// Posted (by the window-event hooks) to re-check whether an app covers the screen.
const WM_CHECK_COVER: u32 = WM_APP + 11;
/// Blinks the caret in the Wi-Fi password field.
const CARET_TIMER: usize = 9;

const WIFI_BLUE: u32 = 0x0A84FF;
// System page ring colours
const CPU_PINK: u32 = 0xFF375F;
const RAM_LIME: u32 = 0xA3E635;
const GPU_PURPLE: u32 = 0xBF5AF2;
const SSD_CYAN: u32 = 0x32ADE6;

/// The camera/mic bubble waits this long before popping up (no flicker on a quick pass).
const BUBBLE_TIMER: usize = 5;
const BUBBLE_DELAY_MS: u32 = 350;

const WM_MOUSEHWHEEL: u32 = 0x020E;
/// Wheel/touchpad distance that switches one page (120 = one wheel notch).
const SCROLL_STEP: i32 = 60;

/// Something clickable in the open card.
#[derive(Clone, PartialEq)]
enum Action {
    Tab(String),
    Previous,
    PlayPause,
    Next,
    Shuffle,
    Repeat,
    Seek { x0: f32, x1: f32 },
    /// Hovering the camera (privacy::CAMERA) or mic (privacy::MIC) indicator: show who's using it.
    Privacy(u8),
    /// The click strips at the card's left / right edges.
    PagePrev,
    PageNext,
    /// Controls page: the sliders (press and drag), mute, the device pickers and their rows.
    Volume { x0: f32, x1: f32 },
    Brightness { x0: f32, x1: f32 },
    Mute,
    Picker { input: bool },
    Device { id: String, input: bool },
    /// Network page: open/close the Wi-Fi (bt = false) or Bluetooth list; rescan; pick one.
    NetList { bt: bool },
    WifiRefresh,
    Wifi(String),
    Bt(String),
    /// The on/off switch on the Wi-Fi (bt = false) or Bluetooth tile.
    Radio { bt: bool },
    /// The pin in the card's top-right corner: keeps the notch open.
    Pin,
    /// Shelf page: a file tile (press and drag it out), its remove button, "Clear all".
    ShelfItem(usize),
    ShelfRemove(usize),
    ShelfClear,
    /// Wi-Fi password row: the text field, show/hide, Join.
    PwField,
    PwShow,
    PwJoin,
    /// The gear on Home (opens Settings) and Settings' Done button.
    OpenSettings,
    CloseSettings,
    SettingsTab(usize),
    Startup,
    AlwaysShow,
    /// Settings > Motion > Animation speed option.
    Speed(usize),
    /// Settings > Motion > Hover delay box (click to type in it).
    DelayBox,
    /// Settings > Motion > Open on: Hover / Click / Both.
    OpenMode(usize),
    /// Settings: a page's show/hide switch, and its drag handle (press and drag to reorder).
    TabToggle(Page),
    TabGrip(usize),
}

/// The pages of the open card, left to right.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Home,
    Music,
    Shelf,
    Controls,
    Network,
    System,
}

/// What the closed notch's slider shows after a key press.
#[derive(Clone, Copy, PartialEq)]
enum Hud {
    Volume,
    Brightness,
}

impl Action {
    /// Buttons you can click (hand cursor); the privacy indicators are hover-only.
    fn clickable(&self) -> bool {
        !matches!(self, Action::Privacy(_))
    }
}

struct App {
    hwnd: HWND,
    dpi_scale: f32,
    // Drawing resources (the factory must outlive the render target)
    factory: ID2D1Factory,
    target: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    gfx: Gfx,
    mem_dc: HDC,
    bitmap: HBITMAP,
    px_w: i32,
    px_h: i32,
    // Frame timing for the monitor the notch is on
    refresh_hz: f32,
    /// That monitor's DXGI output, used to wait for its vertical blank (its exact refresh).
    vblank_output: Option<IDXGIOutput>,
    // Animation state
    width: Spring,
    height: Spring,
    /// Time of the previous frame; None means the next frame is the first of an animation.
    last_frame: Option<Instant>,
    animating: bool,
    hovered: bool,
    // Now Playing
    media: Media,
    sessions: Vec<Session>,
    /// The session the notch is showing (see `apply_snapshot` for how it's chosen).
    selected: Option<String>,
    /// Album art uploaded to Direct2D: art key -> (big, small).
    art: HashMap<u64, (ID2D1Bitmap, ID2D1Bitmap)>,
    started: Instant,
    tick_ms: u32,
    // Mouse
    hits: Vec<(D2D_RECT_F, Action)>,
    hover_action: Option<Action>,
    /// A short message shown in place of the artist, e.g. "Apple Music doesn't let…".
    notice: Option<String>,
    // Idle info
    clock: sysinfo::Clock,
    battery: Option<sysinfo::Battery>,
    /// privacy::CAMERA | privacy::MIC while an app uses them.
    privacy: u8,
    /// Which apps use the camera/mic (filled by privacy.rs).
    privacy_info: Option<std::sync::Arc<privacy::Shared>>,
    // Pages (0 = Home, 1 = Music when something is loaded)
    page: usize,
    /// Animated page position: 0.0 = Home, 1.0 = Music; drives the slide.
    page_pos: Ease,
    wheel: i32,
    last_page_switch: Instant,
    // Camera/mic bubble
    bubble: Spring,
    bubble_flag: Option<u8>,
    bubble_pending: Option<u8>,
    bubble_anchor: D2D_RECT_F,
    // Controls page
    audio: audio::Audio,
    sound: audio::State,
    brightness: brightness::Brightness,
    /// Built-in display brightness 0–100; None until known.
    bright: Option<u8>,
    /// False when the display's brightness can't be set (desktop monitor).
    bright_supported: bool,
    /// Which device list is open: Some(false) = outputs, Some(true) = inputs.
    picker: Option<bool>,
    /// The slider being dragged.
    dragging: Option<Action>,
    // Volume / brightness slider in the closed notch
    hud: Option<Hud>,
    hud_kind: Hud,
    /// 0 = clock, 1 = slider (crossfade).
    hud_mix: Spring,
    /// Closed notch: 0 = clock, 1 = now playing (album art + waveform), while music plays.
    music_mix: Spring,
    /// Music is playing or paused (keeps the notch out of the thin bar).
    music: bool,
    /// The pointer left the open notch; it closes when LEAVE_TIMER fires unless it returns.
    leaving: bool,
    /// Windows is watching for the pointer leaving (TrackMouseEvent is armed).
    tracking: bool,
    /// The pointer arrived and the hover delay is running (HOVER_TIMER).
    hover_pending: bool,
    /// Typing in the hover delay box: what's typed so far (None = not editing).
    delay_edit: Option<String>,
    /// Pinned open: doesn't close when the pointer leaves.
    pinned: bool,
    /// A full-screen or maximised app is in front: shrink to a thin bar.
    covered: bool,
    /// A full-screen app is in front: the notch is hidden, its drawing surfaces freed.
    fullscreen: bool,
    /// When the pointer arrived on the bar (it opens after the hover delay).
    dwell_since: Option<Instant>,
    /// The window lets clicks through (bar mode).
    click_through: bool,
    // Network page
    wifi: wifi::Wifi,
    wifi_state: wifi::State,
    bt: bluetooth::Bluetooth,
    bt_state: bluetooth::State,
    /// Which list is open: Some(false) = Wi-Fi, Some(true) = Bluetooth.
    net_list: Option<bool>,
    radios: radios::Radios,
    radio_state: radios::State,
    // System page
    stats: stats::Monitor,
    sys: stats::Stats,
    // File shelf
    shelf: Vec<String>,
    shelf_art: HashMap<String, ID2D1Bitmap>,
    thumbs: shelf::Thumbs,
    /// Files are being dragged over the notch.
    dropping: bool,
    /// 0 = normal, 1 = drop zone (crossfade).
    drop_mix: Spring,
    /// Pressed on a shelf tile at (x, y): dragging far enough starts a drag out.
    shelf_press: Option<(usize, f32, f32)>,
    /// We're dragging a file out (ignore our own drop zone).
    dragging_out: bool,
    /// First visible shelf tile (scroll).
    shelf_scroll: usize,
    // Wi-Fi password field
    /// The network whose password row is open.
    pw_ssid: Option<String>,
    pw_text: String,
    pw_show: bool,
    /// The field has the keyboard (the notch window is active while typing).
    pw_focus: bool,
    caret_on: bool,
    // Settings
    prefs: settings::Prefs,
    /// The Settings view is showing (instead of the pages).
    in_settings: bool,
    /// Which Settings tab is shown (SETTINGS_TABS).
    settings_tab: usize,
    /// "Launch at startup", as the registry says (read when Settings opens).
    startup: bool,
    /// Reordering: the row being dragged (index into prefs.order).
    reorder: Option<usize>,
    /// Each Settings row's animated top (design pixels), so rows glide into new places.
    row_y: Vec<(Page, Spring)>,
    /// Where on the grabbed row the pointer holds it (design pixels from its top).
    grab_offset: f32,
    // Shadow under the notch (a separate click-through window)
    shadow_hwnd: HWND,
    shadow_dc: HDC,
    shadow_bitmap: HBITMAP,
    /// The shadow bitmap's pixels (BGRA, top-down rows), written by paint_shadow.
    shadow_bits: *mut u32,
    /// Notch size the shadow was last drawn for (redrawn only when it changes).
    shadow_key: (i32, i32),
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static NOTCH_HWND: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    static SHADOW_HWND: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

/// Put the notch back at the very top of the always-on-top windows, without moving,
/// resizing or activating it.
unsafe fn stay_on_top(hwnd: HWND) {
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING,
    );
    // The shadow sits directly below the notch.
    let shadow = SHADOW_HWND.with(|h| h.get());
    if shadow != 0 {
        let _ = SetWindowPos(
            HWND(shadow as *mut _),
            Some(hwnd),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING,
        );
    }
}

/// The shadow window does nothing itself.
extern "system" fn shadow_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Called by Windows whenever another app comes to the front.
unsafe extern "system" fn on_foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let raw = NOTCH_HWND.with(|h| h.get());
    if raw != 0 {
        stay_on_top(HWND(raw as *mut _));
        let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_CHECK_COVER, WPARAM(0), LPARAM(0));
    }
}

/// Called by Windows when a window moves/resizes or is minimised/restored. Only the
/// active window matters (did it just go full screen / maximised?).
unsafe extern "system" fn on_window_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    const OBJID_WINDOW: i32 = 0;
    if object != OBJID_WINDOW || hwnd != GetForegroundWindow() {
        return;
    }
    let raw = NOTCH_HWND.with(|h| h.get());
    if raw != 0 {
        let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_CHECK_COVER, WPARAM(0), LPARAM(0));
    }
}

/// Is the active window full screen or maximised on the notch's monitor?
/// (The desktop and taskbar don't count.)
unsafe fn app_covers_screen(notch: HWND) -> Cover {
    let fg = GetForegroundWindow();
    if fg.is_invalid() || fg == notch || !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() {
        return Cover::None;
    }
    let mut class = [0u16; 64];
    let len = GetClassNameW(fg, &mut class).max(0) as usize;
    let class = String::from_utf16_lossy(&class[..len]);
    if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return Cover::None;
    }
    let monitor = MonitorFromWindow(notch, MONITOR_DEFAULTTOPRIMARY);
    if MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) != monitor {
        return Cover::None;
    }
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let mut r = RECT::default();
    if !GetMonitorInfoW(monitor, &mut info).as_bool() || GetWindowRect(fg, &mut r).is_err() {
        return Cover::None;
    }
    let caption = GetWindowLongW(fg, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0;
    cover_kind(r, info.rcMonitor, IsZoomed(fg).as_bool(), caption)
}

/// What the window in front does to the notch's screen.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Cover {
    None,
    /// Maximised: the taskbar still shows. The notch shrinks to the thin bar.
    Maximized,
    /// Full screen (a game, a video, F11): covers the whole monitor. The notch hides.
    FullScreen,
}

/// Full screen = covers the whole monitor and isn't an ordinary maximised window (one
/// with a title bar can cover it all when the taskbar auto-hides).
fn cover_kind(window: RECT, monitor: RECT, zoomed: bool, caption: bool) -> Cover {
    let covers = window.left <= monitor.left && window.top <= monitor.top && window.right >= monitor.right && window.bottom >= monitor.bottom;
    if covers && !(zoomed && caption) {
        Cover::FullScreen
    } else if zoomed || covers {
        Cover::Maximized
    } else {
        Cover::None
    }
}

fn main() -> Result<()> {
    unsafe {
        // Only one copy at a time: if it's already running, quietly exit.
        // (The handle is never closed, so the lock lasts as long as this process.)
        let _single_instance = CreateMutexW(None, true, w!("Local\\DynamicIslandForWindows"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

        // "Launch at startup" keeps working if this folder was moved.
        startup::refresh_path();

        // Crisp rendering on high-DPI screens, per monitor.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("DynamicIslandWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }

        // Layered = per-pixel alpha, Topmost = above other windows,
        // ToolWindow = hidden from taskbar and Alt+Tab, NoActivate = never steals focus.
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Dynamic Island"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?;

        // Drag and drop (the file shelf) needs OLE on this thread.
        let _ = windows::Win32::System::Ole::OleInitialize(None);

        // The notch's soft shadow lives in its own window just below it, which lets every
        // click through (so the shadow never blocks what's under it).
        let shadow_class = w!("DynamicIslandShadow");
        let shadow_wc = WNDCLASSW { lpfnWndProc: Some(shadow_proc), hInstance: instance, lpszClassName: shadow_class, ..Default::default() };
        RegisterClassW(&shadow_wc);
        let shadow = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            shadow_class,
            w!("Dynamic Island shadow"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )
        .unwrap_or_default();
        SHADOW_HWND.with(|h| h.set(shadow.0 as isize));

        let app = App::new(hwnd, shadow)?;
        APP.with(|a| *a.borrow_mut() = Some(app));
        with_app(|app| {
            app.position_window();
            app.refresh_display_info();
            let _ = app.render();
        });

        let _ = ShowWindow(shadow, SW_SHOWNOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        stay_on_top(hwnd);

        // Stay above other always-on-top windows: re-assert whenever the active app changes,
        // plus a cheap check every 2 seconds as a backup.
        NOTCH_HWND.with(|h| h.set(hwnd.0 as isize));
        let _hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        // Full-screen / maximised check: when the active window resizes or is minimised.
        let _hook_move = SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            None,
            Some(on_window_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let _hook_min = SetWinEventHook(
            EVENT_SYSTEM_MINIMIZESTART,
            EVENT_SYSTEM_MINIMIZEEND,
            None,
            Some(on_foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let _ = PostMessageW(Some(hwnd), WM_CHECK_COVER, WPARAM(0), LPARAM(0));
        SetTimer(Some(hwnd), TOPMOST_TIMER, 2000, None);

        // Clock updates exactly on the minute; camera/mic changes are pushed by Windows.
        SetTimer(Some(hwnd), CLOCK_TIMER, sysinfo::ms_to_next_minute(), None);
        let privacy_info = privacy::start(hwnd);
        shelf::register(hwnd, shelf::DropTarget::new(|event| with_app(|app| app.on_drop_event(event)).unwrap_or(false)));
        brightness::watch(hwnd); // brightness changes arrive as WM_POWERBROADCAST
        with_app(|app| app.privacy_info = Some(privacy_info));

        // Tray icon with a Quit menu. If the taskbar isn't available, carry on without it.
        let _tray = tray::Tray::create(instance).ok();

        run_message_loop();

        APP.with(|a| a.borrow_mut().take()); // drop resources before exit
        Ok(())
    }
}

/// The main loop.
/// - Idle: sleep inside GetMessageW until something happens (0% CPU).
/// - Animating: handle waiting messages, draw a frame, then block until the next refresh
///   of the monitor the notch is on. One frame per refresh, whatever the refresh rate.
unsafe fn run_message_loop() {
    let mut msg = MSG::default();
    loop {
        let animating = with_app(|app| app.animating).unwrap_or(false);

        if animating {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let frame_start = Instant::now();
            with_app(|app| app.tick());
            wait_for_next_refresh(frame_start);
        } else {
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                return; // WM_QUIT
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Block until the monitor's next refresh.
unsafe fn wait_for_next_refresh(frame_start: Instant) {
    let (output, refresh_hz) =
        with_app(|app| (app.vblank_output.clone(), app.refresh_hz)).unwrap_or((None, 60.0));

    // Best: wait for this exact monitor's vertical blank. Works with any refresh rate,
    // mixed-refresh multi-monitor setups, and laptops that change refresh rate on the fly.
    let synced = match output {
        Some(output) if output.WaitForVBlank().is_ok() => true,
        Some(_) => {
            // Monitor unplugged or driver reset: stop using it until displays change.
            with_app(|app| app.vblank_output = None);
            false
        }
        None => false,
    };
    // Fallback: wait for the desktop compositor's next frame.
    let synced = synced || DwmFlush().is_ok();

    // Safety net: some setups (VMs, Remote Desktop) return from these waits instantly.
    // Never let that turn into a CPU-burning loop.
    let frame_time = Duration::from_secs_f32(1.0 / refresh_hz);
    if !synced || frame_start.elapsed() < Duration::from_millis(1) {
        std::thread::sleep(frame_time.saturating_sub(frame_start.elapsed()));
    }
}

/// Find the DXGI output (graphics-card connector) driving `monitor`. Searches every
/// graphics card, so it also works on laptops with both integrated and dedicated GPUs.
unsafe fn find_dxgi_output(monitor: HMONITOR) -> Option<IDXGIOutput> {
    let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
    let mut adapter_index = 0;
    while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
        let mut output_index = 0;
        while let Ok(output) = adapter.EnumOutputs(output_index) {
            if output.GetDesc().is_ok_and(|desc| desc.Monitor == monitor) {
                return Some(output);
            }
            output_index += 1;
        }
        adapter_index += 1;
    }
    None
}

/// The monitor's current refresh rate in Hz, as Windows reports it.
unsafe fn monitor_refresh_hz(monitor: HMONITOR) -> Option<f32> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if !GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO).as_bool() {
        return None;
    }
    let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    if !EnumDisplaySettingsW(PCWSTR(info.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode)
        .as_bool()
    {
        return None;
    }
    // 0 and 1 mean "hardware default"; treat anything implausible as unknown.
    let hz = mode.dmDisplayFrequency;
    (hz >= 20).then_some(hz as f32)
}

/// Run `f` on the app state, if it exists and isn't already borrowed.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

/// The Direct2D drawing surface. We draw on the CPU ("software"): for one small shape
/// it's faster than drawing on the GPU and copying the result back every frame, it works
/// the same on every machine, and it can't be lost when the graphics driver resets.
unsafe fn create_render_target(
    factory: &ID2D1Factory,
) -> Result<(ID2D1DCRenderTarget, ID2D1SolidColorBrush)> {
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0, // we do our own DPI scaling
        dpiY: 96.0,
        ..Default::default()
    };
    let target = factory.CreateDCRenderTarget(&props)?;
    target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
    // Grayscale text: ClearType needs an opaque background, and ours is see-through.
    target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    let black = D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    let brush = target.CreateSolidColorBrush(&black, None)?;
    Ok((target, brush))
}

/// The notch's outline in pixels: a box hanging from the top edge, centred at `cx`,
/// `w` wide and `h` tall, with round bottom corners of radius `r`. Its top corners flare
/// out into the screen edge over `shoulder` pixels.
struct Outline {
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
fn erfc(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = t * (0.254_829_6 + t * (-0.284_496_7 + t * (1.421_413_7 + t * (-1.453_152_1 + t * 1.061_405_4))));
    let e = poly * (-x * x).exp();
    if x >= 0.0 { e } else { 2.0 - e }
}

fn length(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

/// How the shadow looks, in pixels: blur width, darkness at the edge, shift down.
struct ShadowLook {
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
fn paint_shadow(pixels: &mut [u32], width: usize, height: usize, notch: &Outline, look: &ShadowLook) {
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
fn dither(x: usize, y: usize) -> f32 {
    let mut n = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    n ^= n >> 15;
    n = n.wrapping_mul(0x2C1B_3C6D);
    n ^= n >> 12;
    (n & 0xFFFF) as f32 / 65536.0
}

/// Bars in the closed now-playing waveform.
const WAVE_BARS: usize = 6;

/// Height of waveform bar `i` (0–1 of its full height) at `time` seconds: taller in the
/// middle, each bar moving at its own pace; low and still when paused.
fn wave_level(i: usize, time: f32, playing: bool) -> f32 {
    const ENVELOPE: [f32; WAVE_BARS] = [0.45, 0.75, 1.0, 0.85, 0.6, 0.4];
    const SPEEDS: [f32; WAVE_BARS] = [6.1, 8.3, 7.2, 9.4, 6.8, 7.9];
    const PHASES: [f32; WAVE_BARS] = [0.0, 2.1, 0.9, 3.3, 1.6, 2.7];
    if !playing {
        return 0.15;
    }
    ENVELOPE[i] * (0.35 + 0.65 * (0.5 + 0.5 * (time * SPEEDS[i] + PHASES[i]).sin()).powf(1.3))
}

/// Where page `i` sits relative to the slide position `pos`, in pages, with the pages in a
/// ring of `count`: the shortest way round, in (-count/2, count/2].
fn ring_offset(i: usize, pos: f32, count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let n = count as f32;
    let d = (i as f32 - pos).rem_euclid(n);
    if d > n / 2.0 { d - n } else { d }
}

/// Which media session the notch shows (and sends commands to). The one you picked stays
/// until it stops or goes away: a new app starting to play, or Windows' "current" app
/// changing, doesn't switch it (the source tabs show those). With none, it picks what's
/// playing (Windows' current first), else what's paused.
fn pick_source(selected: Option<&str>, sessions: &[(&str, Status)], system: Option<&str>) -> Option<String> {
    let is = |id: &str, ok: &dyn Fn(Status) -> bool| sessions.iter().any(|(s, st)| *s == id && ok(*st));
    let live = |st: Status| st != Status::Stopped;
    let playing = |st: Status| st == Status::Playing;
    if let Some(id) = selected.filter(|id| is(id, &live)) {
        return Some(id.to_string());
    }
    let first = |ok: &dyn Fn(Status) -> bool| sessions.iter().find(|(_, st)| ok(*st)).map(|(id, _)| *id);
    system
        .filter(|id| is(id, &playing))
        .or_else(|| first(&playing))
        .or(system.filter(|id| is(id, &live)))
        .or_else(|| first(&live))
        .map(str::to_string)
}

fn format_time(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn contains(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

impl App {
    unsafe fn new(hwnd: HWND, shadow: HWND) -> Result<Self> {
        let factory: ID2D1Factory =
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let (target, brush) = create_render_target(&factory)?;
        let dpi_scale = GetDpiForWindow(hwnd) as f32 / 96.0;
        let gfx = Gfx::new(&factory, dpi_scale)?;

        let mut app = App {
            hwnd,
            dpi_scale,
            factory,
            target,
            brush,
            gfx,
            mem_dc: CreateCompatibleDC(None),
            bitmap: HBITMAP::default(),
            px_w: 0,
            px_h: 0,
            refresh_hz: 60.0,
            vblank_output: None,
            width: Spring::new(CLOSED.0),
            height: Spring::new(CLOSED.1),
            last_frame: None,
            animating: false,
            hovered: false,
            media: Media::start(hwnd),
            sessions: Vec::new(),
            selected: None,
            art: HashMap::new(),
            started: Instant::now(),
            tick_ms: 0,
            hits: Vec::new(),
            hover_action: None,
            notice: None,
            clock: sysinfo::clock(),
            battery: sysinfo::battery(),
            privacy: 0,
            privacy_info: None,
            page: 0,
            page_pos: Ease::new(0.0),
            wheel: 0,
            last_page_switch: Instant::now(),
            bubble: Spring::new(0.0),
            bubble_flag: None,
            bubble_pending: None,
            bubble_anchor: D2D_RECT_F::default(),
            audio: audio::Audio::start(hwnd),
            sound: audio::State::default(),
            brightness: brightness::Brightness::start(hwnd),
            bright: None,
            bright_supported: true,
            picker: None,
            dragging: None,
            hud: None,
            hud_kind: Hud::Volume,
            hud_mix: Spring::new(0.0),
            music_mix: Spring::new(0.0),
            music: false,
            leaving: false,
            tracking: false,
            hover_pending: false,
            delay_edit: None,
            pinned: false,
            covered: false,
            fullscreen: false,
            dwell_since: None,
            click_through: false,
            wifi: wifi::Wifi::start(hwnd),
            wifi_state: wifi::State::default(),
            bt: bluetooth::Bluetooth::start(hwnd),
            bt_state: bluetooth::State::default(),
            net_list: None,
            radios: radios::Radios::start(hwnd),
            radio_state: radios::State::default(),
            stats: stats::Monitor::start(hwnd),
            sys: stats::Stats::default(),
            shelf: Vec::new(),
            shelf_art: HashMap::new(),
            thumbs: shelf::Thumbs::start(hwnd),
            dropping: false,
            drop_mix: Spring::new(0.0),
            shelf_press: None,
            dragging_out: false,
            shelf_scroll: 0,
            pw_ssid: None,
            pw_text: String::new(),
            pw_show: false,
            pw_focus: false,
            caret_on: true,
            prefs: settings::Prefs::load(),
            in_settings: false,
            settings_tab: TAB_GENERAL,
            startup: false,
            reorder: None,
            row_y: Vec::new(),
            grab_offset: 0.0,
            shadow_hwnd: shadow,
            shadow_dc: CreateCompatibleDC(None),
            shadow_bitmap: HBITMAP::default(),
            shadow_bits: std::ptr::null_mut(),
            shadow_key: (-1, -1),
        };
        app.resize_surface()?;
        Ok(app)
    }

    /// (Re)create the off-screen bitmap at the current DPI.
    unsafe fn resize_surface(&mut self) -> Result<()> {
        self.px_w = (CANVAS_W * self.dpi_scale).round() as i32;
        self.px_h = (CANVAS_H * self.dpi_scale).round() as i32;

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.px_w,
                biHeight: -self.px_h, // negative = top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(self.mem_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        SelectObject(self.mem_dc, bitmap.into());
        if !self.bitmap.is_invalid() {
            let _ = DeleteObject(self.bitmap.into());
        }
        self.bitmap = bitmap;

        let mut bits = std::ptr::null_mut();
        let shadow = CreateDIBSection(Some(self.shadow_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        SelectObject(self.shadow_dc, shadow.into());
        if !self.shadow_bitmap.is_invalid() {
            let _ = DeleteObject(self.shadow_bitmap.into());
        }
        self.shadow_bitmap = shadow;
        self.shadow_bits = bits.cast();
        self.shadow_key = (-1, -1);
        Ok(())
    }

    /// Put the window at the top centre of the monitor it's on.
    unsafe fn position_window(&mut self) {
        let monitor = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let screen = info.rcMonitor;
        let x = screen.left + (screen.right - screen.left - self.px_w) / 2;
        let y = screen.top;
        let _ = SetWindowPos(
            self.hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            self.px_w,
            self.px_h,
            SWP_NOACTIVATE,
        );
        if !self.shadow_hwnd.is_invalid() {
            let _ = SetWindowPos(self.shadow_hwnd, Some(self.hwnd), x, y, self.px_w, self.px_h, SWP_NOACTIVATE);
        }
        self.shadow_key = (-1, -1);
    }

    /// Look up the refresh rate and vblank source of the monitor the notch is on.
    /// Called at start-up and whenever displays change.
    unsafe fn refresh_display_info(&mut self) {
        let monitor = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTOPRIMARY);
        self.refresh_hz = monitor_refresh_hz(monitor).unwrap_or(60.0);
        self.vblank_output = find_dxgi_output(monitor);
    }

    // ---------------------------------------------------------------- Now Playing state

    fn current(&self) -> Option<&Session> {
        let id = self.selected.as_ref()?;
        self.sessions.iter().find(|s| &s.id == id)
    }

    fn has_media(&self) -> bool {
        self.current().is_some_and(|s| s.status != Status::Stopped)
    }

    /// Tabs: the shown session first, then every other app with media loaded (greyed out),
    /// playing ones before paused ones.
    fn tabs(&self) -> Vec<&Session> {
        let mut tabs: Vec<&Session> = self.current().into_iter().collect();
        let others = |status: Status| {
            self.sessions
                .iter()
                .filter(move |s| s.status == status && Some(&s.id) != self.selected.as_ref())
        };
        tabs.extend(others(Status::Playing));
        tabs.extend(others(Status::Paused));
        tabs
    }

    /// A new picture of what's playing arrived from media.rs. Which session it shows: see pick_source.
    unsafe fn apply_snapshot(&mut self, snapshot: MediaSnapshot) {
        let had_media = self.has_media();
        let page_before = self.page_kind();
        if let Some(notice) = snapshot.notice {
            self.notice = Some(notice);
            SetTimer(Some(self.hwnd), NOTICE_TIMER, NOTICE_MS, None);
        }
        self.sessions = snapshot.sessions;

        let ids: Vec<(&str, Status)> = self.sessions.iter().map(|s| (s.id.as_str(), s.status)).collect();
        self.selected = pick_source(self.selected.as_deref(), &ids, snapshot.system_current.as_deref());

        // Upload album art for the shown session; forget art no longer needed.
        let wanted = self.current().and_then(|s| s.art.clone());
        self.art.retain(|key, _| wanted.as_ref().is_some_and(|a| a.key == *key));
        if let Some(art) = wanted {
            if !self.art.contains_key(&art.key) {
                let t: ID2D1RenderTarget = self.target.clone().into();
                if let (Ok(big), Ok(small)) = (
                    gfx::bitmap(&t, &art.big, art.big_size),
                    gfx::bitmap(&t, &art.small, art.small_size),
                ) {
                    self.art.insert(art.key, (big, small));
                }
            }
        }


        // The Music page comes and goes with media: stay on the same page if it still exists.
        match self.pages().iter().position(|p| *p == page_before) {
            Some(i) => {
                if i != self.page {
                    self.page = i;
                    self.page_pos = Ease::new(i as f32);
                }
                if had_media != self.has_media() {
                    self.set_expanded(self.hovered);
                }
            }
            None => self.switch_page(0), // the Music page went away
        }
        self.update_ticker();
        let _ = self.render();
    }

    /// Redraw on a timer while music plays: waveform when closed, progress bar when open.
    unsafe fn update_ticker(&mut self) {
        let playing = self.current().is_some_and(|s| s.status == Status::Playing);
        // The closed notch cross-fades between the clock and now playing; paused music stays.
        let loaded = self.current().is_some_and(|s| s.status != Status::Stopped);
        let mix = if loaded { 1.0 } else { 0.0 };
        // Music playing or paused: never the thin bar (re-checked when that changes).
        if loaded != self.music {
            self.music = loaded;
            if self.covered && !self.hovered {
                self.set_bar_mode(true);
                self.set_expanded(false); // music set first, so this doesn't loop back here
            }
        }
        if self.music_mix.target != mix {
            self.music_mix.set_target(mix, spring::ROW);
            self.start_animation();
        }
        // ~30 fps while music plays: the waveform is visible closed and on every open page
        // (the progress bar rides along).
        let ms = if playing { 33 } else { 0 };
        if ms != self.tick_ms {
            self.tick_ms = ms;
            if ms == 0 {
                let _ = KillTimer(Some(self.hwnd), TICK_TIMER);
            } else {
                SetTimer(Some(self.hwnd), TICK_TIMER, ms, None);
            }
        }
    }

    // ---------------------------------------------------------------- drawing

    fn closed_size(&self) -> (f32, f32) {
        if self.hud.is_some() {
            HUD
        } else if self.covered && !self.prefs.always_show && !self.music {
            BAR
        } else if !self.shelf.is_empty() {
            (CLOSED.0 + SHELF_CLOSED_EXTRA, CLOSED.1)
        } else {
            CLOSED
        }
    }

    fn open_size(&self) -> (f32, f32) {
        if self.in_settings {
            return (CARD_W, self.settings_height());
        }
        let h = match self.page_kind() {
            Page::Home => HOME_H,
            Page::Music => MUSIC_H,
            Page::Shelf => SHELF_H,
            Page::Controls => CONTROLS_H + self.device_list_height().map_or(0.0, |h| h + 8.0),
            Page::Network => NET_H + self.net_list_height().map_or(0.0, |h| h + 8.0),
            Page::System => SYSTEM_H,
        };
        (CARD_W, h)
    }

    /// Home, Music while an app has something loaded, then Controls.
    fn pages(&self) -> Vec<Page> {
        // Home first, then the rest in your order (Settings), skipping hidden ones and
        // Music / Shelf while they have nothing to show.
        let mut pages = vec![Page::Home];
        for &page in &self.prefs.order {
            let has_content = match page {
                Page::Music => self.has_media(),
                Page::Shelf => !self.shelf.is_empty(),
                _ => true,
            };
            if has_content && !self.prefs.hidden.contains(&page) {
                pages.push(page);
            }
        }
        pages
    }

    /// Rows in the open Wi-Fi / Bluetooth list (capped), and the list's height.
    fn net_rows(&self) -> Option<usize> {
        let bt = self.net_list?;
        let on = if bt { self.radio_state.bluetooth } else { self.radio_state.wifi }.unwrap_or(true);
        let n = match (on, bt) {
            (false, _) => 1, // just "… is off"
            (true, true) => self.bt_state.devices.len(),
            (true, false) => self.wifi_state.networks.len(),
        };
        Some(n.clamp(1, MAX_DEVICES))
    }

    fn net_list_height(&self) -> Option<f32> {
        Some(LIST_HEADER + self.net_rows()? as f32 * 34.0 + 8.0 + if self.password_row_shown() { PASSWORD_ROW } else { 0.0 })
    }

    /// Is the password row showing (its network is in the visible part of the list)?
    fn password_row_shown(&self) -> bool {
        self.net_list == Some(false)
            && self.radio_state.wifi != Some(false)
            && self.pw_ssid.as_ref().is_some_and(|ssid| self.wifi_state.networks.iter().take(MAX_DEVICES).any(|n| &n.ssid == ssid))
    }

    /// The System page samples CPU/GPU only while it's on screen.
    fn update_stats_wanted(&self) {
        self.stats.set_wanted(self.hovered && self.page_kind() == Page::System);
    }

    fn page_count(&self) -> usize {
        self.pages().len()
    }

    fn page_kind(&self) -> Page {
        let pages = self.pages();
        pages[self.page.min(pages.len() - 1)]
    }

    /// The open device list: which devices, and which one is the default.
    fn picker_devices(&self) -> Option<(bool, Vec<audio::Device>, Option<String>)> {
        let input = self.picker?;
        let (list, default) = if input {
            (&self.sound.inputs, &self.sound.default_input)
        } else {
            (&self.sound.outputs, &self.sound.default_output)
        };
        Some((input, list.iter().take(MAX_DEVICES).cloned().collect(), default.clone()))
    }

    /// Height of the open device list, if one is open.
    fn device_list_height(&self) -> Option<f32> {
        let (_, devices, _) = self.picker_devices()?;
        Some(12.0 + devices.len().max(1) as f32 * 34.0)
    }

    /// How open the notch is: 0 = closed, 1 = open. Measured against the shortest page, so
    /// sliding to a taller page never fades the content.
    fn openness(&self) -> f32 {
        let closed = self.closed_size().1;
        (self.height.value - closed) / (SHORTEST_PAGE_H - closed).max(1.0)
    }

    /// Draw the notch and push the pixels to the screen.
    unsafe fn render(&mut self) -> Result<()> {
        if self.fullscreen {
            return Ok(()); // hidden behind a full-screen app, nothing to draw into
        }
        self.render_shadow();
        let rect_px = RECT { left: 0, top: 0, right: self.px_w, bottom: self.px_h };
        self.target.BindDC(self.mem_dc, &rect_px)?;

        let notch = self.notch_geometry()?;
        let t: ID2D1RenderTarget = self.target.clone().into();
        let brush: ID2D1Brush = self.brush.clone().into();

        t.BeginDraw();
        t.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        self.brush.SetColor(&rgb(0x000000, 1.0));
        self.brush.SetOpacity(1.0);
        t.FillGeometry(&notch, &brush, None);

        // Content, clipped to the notch so it's revealed as the notch grows.
        let s = self.dpi_scale;
        let w = self.width.value.max(1.0) * s;
        let h = self.height.value.max(1.0) * s;
        let cx = self.px_w as f32 / 2.0;
        t.PushAxisAlignedClip(&rect(cx - w / 2.0, 0.0, cx + w / 2.0, h), D2D1_ANTIALIAS_MODE_ALIASED);
        let open = self.openness().clamp(0.0, 1.0);
        // The closed content fades out as the notch shrinks to the thin bar.
        let shown = ((self.height.value - BAR.1) / (CLOSED.1 - BAR.1)).clamp(0.0, 1.0);
        let closed_alpha = (1.0 - open / 0.35).clamp(0.0, 1.0) * shown;
        let open_alpha = ((open - 0.55) / 0.45).clamp(0.0, 1.0);
        self.hits.clear();
        let drop = self.drop_mix.value.clamp(0.0, 1.0);
        let (closed_alpha, open_alpha) = (closed_alpha * (1.0 - drop), open_alpha * (1.0 - drop));
        if closed_alpha > 0.0 {
            self.draw_closed(&t, &brush, closed_alpha);
        }
        if open_alpha > 0.0 {
            self.draw_pages(&t, &brush, open_alpha);
        }
        if drop > 0.01 {
            self.draw_drop_zone(&t, &brush, drop);
        }
        t.PopAxisAlignedClip();

        // Camera/mic bubble (below the card), animated by its own spring.
        let pop = self.bubble.value;
        if pop > 0.01 && open_alpha > 0.0 {
            if let Some(flag) = self.bubble_flag {
                let anchor = self.bubble_anchor;
                self.draw_privacy_bubble(&t, &brush, flag, &anchor, pop, open_alpha);
            }
        }

        if let Err(error) = t.EndDraw(None, None) {
            // Rare, but if the drawing surface is lost, rebuild it and draw next frame.
            if error.code() == D2DERR_RECREATE_TARGET {
                (self.target, self.brush) = create_render_target(&self.factory)?;
                self.art.clear();
            }
            return Err(error);
        }

        // Hand the finished bitmap to Windows.
        let size = SIZE { cx: self.px_w, cy: self.px_h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            self.hwnd,
            None,
            None,
            Some(&size),
            Some(self.mem_dc),
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )
    }

    /// The soft shadow under the notch, drawn into the click-through shadow window. Only
    /// redrawn when the notch's size changes (not for every waveform frame).
    unsafe fn render_shadow(&mut self) {
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

    /// Battery ring colour: green charging, red low, yellow battery saver, else white.
    fn battery_colour(b: &sysinfo::Battery) -> u32 {
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
    fn privacy_colours(&self) -> Vec<(u32, Icon, u8)> {
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
    unsafe fn draw_privacy_bubble(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, flag: u8, anchor: &D2D_RECT_F, pop: f32, fade: f32) {
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
    unsafe fn draw_privacy_dots(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, right_x: f32, cy: f32, alpha: f32) {
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
    unsafe fn draw_idle_closed(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
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
    unsafe fn draw_now_playing_closed(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
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
        self.draw_privacy_dots(t, brush, wave_right - wave_w - 8.0 * s, h / 2.0, alpha);
    }

    /// Closed, nothing loaded: [time] [day] [shelf files] … [dots][battery].
    unsafe fn draw_clock_closed(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
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

    /// The waveform inside the open card: grows from the closed notch's size to full size
    /// as the notch opens. `right`/`cy` in pixels.
    unsafe fn draw_open_waveform(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, right: f32, cy: f32, alpha: f32) {
        let s = self.dpi_scale;
        let o = self.openness().clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| (a + (b - a) * o) * s;
        self.draw_waveform(t, brush, right, cy, lerp(2.5, 3.0), lerp(2.0, 2.5), lerp(16.0, 20.0), alpha);
    }

    /// The waveform (as the iPhone's): WAVE_BARS bars in the album's colour, centred on
    /// `cy`, right edge at `right`. Moving while playing, low and still while paused, hidden
    /// when nothing is loaded.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_waveform(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, right: f32, cy: f32, bar_w: f32, gap: f32, max_h: f32, alpha: f32) {
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

    /// Open, nothing playing (design 6A + the red bar from 6D):
    /// [red bar][calendar tile: THU / 26][9:41 AM / Thursday, November] … [camera/mic, centred]
    /// … [84% / Charging][battery ring with bolt].
    unsafe fn draw_idle_open(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
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
    unsafe fn draw_closed(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
        let mix = self.hud_mix.value.clamp(0.0, 1.0);
        if mix < 0.999 {
            self.draw_idle_closed(t, brush, alpha * (1.0 - mix));
        }
        if mix > 0.001 {
            self.draw_hud(t, brush, alpha * mix);
        }
    }

    /// Volume (0–1) and brightness (0–1) as the sliders show them.
    fn level(&self, hud: Hud) -> f32 {
        match hud {
            Hud::Volume => self.sound.volume,
            Hud::Brightness => self.bright.unwrap_or(0) as f32 / 100.0,
        }
    }

    /// A fat rounded slider: dark track, white/yellow fill, icon (and label) on top. The
    /// icon and label are white over the track and black over the fill.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_pill(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, r: D2D_RECT_F, level: f32, fill: u32, icon: Icon, label: &str, alpha: f32) {
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
    unsafe fn draw_hud(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
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
            Hud::Brightness => (BRIGHT_YELLOW, Icon::Sun),
        };
        let pill_h = 22.0 * s;
        let pill = rect(left + 10.0 * s, (h - pill_h) / 2.0, right - 50.0 * s, (h + pill_h) / 2.0);
        self.draw_pill(t, brush, pill, level, fill, icon, "", alpha);
        self.brush.SetColor(&rgb(0xFFFFFF, 1.0));
        self.brush.SetOpacity(alpha);
        let text = format!("{}%", (level * 100.0).round() as u32);
        self.gfx.text(t, &text, &self.gfx.fonts.percent, rect(right - 48.0 * s, 0.0, right - 12.0 * s, h), brush);
    }

    /// Controls page (design H7): Volume and Brightness pills, then the Output and Input
    /// pickers and mute; an open picker lists its devices below.
    unsafe fn draw_controls(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
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
    unsafe fn draw_art(
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
    unsafe fn draw_pages(&mut self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
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
    unsafe fn draw_music(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
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

        // ---- Tabs: the shown app, plus other apps that are playing (greyed out).
        let mut tab_x = 0.0;
        for (i, tab) in self.tabs().into_iter().enumerate() {
            let active = i == 0;
            let font = if active { &self.gfx.fonts.tab_active } else { &self.gfx.fonts.tab };
            let text_w = self.gfx.measure(&tab.app_name, font) / s;
            let tab_w = 10.0 + 6.0 + 6.0 + text_w + 10.0;
            if tab_x + tab_w > w_box {
                break; // no room for more tabs
            }
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

        // ---- Seek bar with times
        let position = session.position_now();
        let progress = if session.duration > 0.0 { (position / session.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let (bar_x0, bar_x1, bar_y) = (128.0, w_box - 48.0, 112.0);
        if session.duration > 0.0 {
            set(0xA1A1AA, 1.0);
            self.gfx.text(t, &format_time(position), &self.gfx.fonts.time, rect(at(84.0), y(104.0), at(124.0), y(120.0)), brush);
            self.gfx.text(t, &format_time(session.duration), &self.gfx.fonts.time_right, rect(at(w_box - 44.0), y(104.0), at(w_box), y(120.0)), brush);
        }
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

    fn settings_height(&self) -> f32 {
        match self.settings_tab {
            TAB_GENERAL => SETTINGS_CONTENT + self.general_rows().len() as f32 * SETTINGS_ROW + 8.0,
            TAB_MOTION => SETTINGS_CONTENT + 3.0 * SETTINGS_ROW + 8.0,
            TAB_PAGES => SETTINGS_TOP + (self.prefs.order.len() + 1) as f32 * SETTINGS_ROW + 8.0,
            _ => SETTINGS_CONTENT + 64.0,
        }
    }

    /// General tab rows: (switch action, label, note, on).
    fn general_rows(&self) -> Vec<(Action, &'static str, &'static str, bool)> {
        vec![
            (Action::Startup, "Launch at startup", "Start with Windows", self.startup),
            (Action::AlwaysShow, "Always show", "Stay full size over full-screen and maximised apps", self.prefs.always_show),
        ]
    }


    /// An iOS-style on/off switch (track `sw`×`sh` at x, centred on cy; design pixels).
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_switch(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, x: f32, cy: f32, on: bool, alpha: f32, ox: f32) {
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
    unsafe fn draw_settings(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
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
    unsafe fn draw_setting_row(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, top: f32, label: &str, note: &str, hot: bool, alpha: f32, ox: f32, w_box: f32) {
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

    /// A file's thumbnail in a rounded square (dark tile behind icons; the extension if
    /// there's no preview yet).
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_file_thumb(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, path: &str, x: f32, y: f32, size: f32, radius: f32, alpha: f32) {
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
    unsafe fn draw_drop_zone(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
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
    unsafe fn draw_shelf(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
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
    unsafe fn draw_network(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
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
    unsafe fn draw_system(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) -> Vec<(D2D_RECT_F, Action)> {
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

    /// Build the notch outline at its current size:
    ///
    ///  ‾‾\              /‾‾   <- shoulders curve out into the top edge of the screen
    ///     |            |
    ///      \__________/       <- rounded bottom corners
    /// The notch's bottom corner radius in pixels: grows as the notch opens.
    fn corner_radius(&self) -> f32 {
        let s = self.dpi_scale;
        let (w, h) = (self.width.value.max(1.0) * s, self.height.value.max(1.0) * s);
        let shoulder = (SHOULDER * s).min(h / 2.0);
        let t = self.openness().clamp(0.0, 1.0);
        ((MIN_CORNER + (MAX_CORNER - MIN_CORNER) * t) * s).min((h - shoulder).max(0.0)).min(w / 2.0)
    }

    unsafe fn notch_geometry(&self) -> Result<ID2D1PathGeometry> {
        let s = self.dpi_scale;
        let w = self.width.value.max(1.0) * s;
        let h = self.height.value.max(1.0) * s;
        let cx = self.px_w as f32 / 2.0;
        let (left, right) = (cx - w / 2.0, cx + w / 2.0);

        let shoulder = (SHOULDER * s).min(h / 2.0); // smaller on the thin bar
        let r = self.corner_radius();
        let (ks, kr) = (BEZIER_CIRCLE * shoulder, BEZIER_CIRCLE * r);

        let pt = |x: f32, y: f32| Vector2 { X: x, Y: y };
        let curve = |p1, p2, p3| D2D1_BEZIER_SEGMENT { point1: p1, point2: p2, point3: p3 };

        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(pt(left - shoulder, 0.0), D2D1_FIGURE_BEGIN_FILLED);
        // Left shoulder
        sink.AddBezier(&curve(
            pt(left - shoulder + ks, 0.0),
            pt(left, shoulder - ks),
            pt(left, shoulder),
        ));
        // Left side, bottom-left corner
        sink.AddLine(pt(left, h - r));
        sink.AddBezier(&curve(pt(left, h - r + kr), pt(left + r - kr, h), pt(left + r, h)));
        // Bottom, bottom-right corner
        sink.AddLine(pt(right - r, h));
        sink.AddBezier(&curve(pt(right - r + kr, h), pt(right, h - r + kr), pt(right, h - r)));
        // Right side, right shoulder
        sink.AddLine(pt(right, shoulder));
        sink.AddBezier(&curve(
            pt(right, shoulder - ks),
            pt(right + shoulder - ks, 0.0),
            pt(right + shoulder, 0.0),
        ));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED); // closes along the top edge
        sink.Close()?;
        Ok(geometry)
    }

    // ---------------------------------------------------------------- animation

    unsafe fn set_expanded(&mut self, expanded: bool) {
        let ((w, h), config) = if self.dropping {
            (DROP, EXPAND)
        } else if expanded {
            (self.open_size(), EXPAND)
        } else {
            (self.closed_size(), COLLAPSE)
        };
        self.width.set_target(w, config);
        self.height.set_target(h, config);
        self.start_animation();
        self.update_ticker();
    }

    /// The message loop sees this and draws one frame per refresh until everything settles.
    fn start_animation(&mut self) {
        if !self.animating {
            self.animating = true;
            self.last_frame = None;
        }
    }

    /// Slide to another page (wraps around), resizing the card to fit it.
    unsafe fn switch_page(&mut self, page: usize) {
        let count = self.page_count();
        self.slide_to(page % count.max(1), None);
    }

    /// One page forward (+1) or back (-1). The pages are a ring: forward from the last
    /// page slides on to Home, back from Home slides on to the last page.
    unsafe fn step_page(&mut self, dir: i32) {
        let count = self.page_count().max(1);
        let page = (self.page as i32 + dir).rem_euclid(count as i32) as usize;
        self.slide_to(page, Some(dir));
    }

    /// Slide to `page`, `dir` pages along the ring (or the shorter way round if None).
    unsafe fn slide_to(&mut self, page: usize, dir: Option<i32>) {
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

    /// Mouse wheel / touchpad on the open notch: one page per step.
    unsafe fn on_scroll(&mut self, delta: i32) {
        if !self.hovered || self.in_settings || self.page_count() < 2 {
            return;
        }
        // Ignore the tail of a fling right after a switch.
        if self.last_page_switch.elapsed() < Duration::from_millis(350) {
            self.wheel = 0;
            return;
        }
        self.wheel += delta;
        if self.wheel.abs() >= SCROLL_STEP {
            let dir = if self.wheel > 0 { 1 } else { -1 };
            self.wheel = 0;
            self.step_page(dir);
        }
    }

    /// Hover target changed: handles the camera/mic bubble's delay and animation.
    unsafe fn set_hover_action(&mut self, action: Option<Action>) {
        if action == self.hover_action {
            return;
        }
        self.hover_action = action.clone();
        match action {
            Some(Action::Privacy(flag)) => {
                if let Some(r) = self.hits.iter().find(|(_, a)| a == &Action::Privacy(flag)).map(|(r, _)| *r) {
                    self.bubble_anchor = r;
                }
                if self.bubble.target > 0.5 {
                    self.bubble_flag = Some(flag); // already showing: just switch
                } else {
                    self.bubble_pending = Some(flag);
                    SetTimer(Some(self.hwnd), BUBBLE_TIMER, BUBBLE_DELAY_MS, None);
                }
            }
            _ => {
                let _ = KillTimer(Some(self.hwnd), BUBBLE_TIMER);
                self.bubble_pending = None;
                if self.bubble.target > 0.0 {
                    self.bubble.set_target(0.0, POP_OUT);
                    self.start_animation();
                }
            }
        }
    }

    /// The hover delay passed while still on the indicator: pop the bubble.
    unsafe fn show_bubble(&mut self) {
        let _ = KillTimer(Some(self.hwnd), BUBBLE_TIMER);
        if let Some(flag) = self.bubble_pending.take() {
            if self.hover_action == Some(Action::Privacy(flag)) {
                self.bubble_flag = Some(flag);
                self.bubble.set_target(1.0, POP_IN);
                self.start_animation();
            }
        }
    }

    /// Draw one animation frame. Called once per screen refresh while animating.
    unsafe fn tick(&mut self) {
        let now = Instant::now();
        // First frame of an animation: assume one refresh has passed, so it starts moving
        // straight away. Cap long gaps (e.g. after a hitch) so nothing jumps.
        let dt = match self.last_frame {
            Some(previous) => (now - previous).as_secs_f32().min(0.05),
            None => 1.0 / self.refresh_hz,
        };
        self.last_frame = Some(now);
        let dt = dt * settings::SPEEDS[self.prefs.speed].1; // Settings > Motion > Animation speed

        self.width.step(dt);
        self.height.step(dt);
        self.page_pos.step(dt);
        self.bubble.step(dt);
        self.hud_mix.step(dt);
        self.music_mix.step(dt);
        self.drop_mix.step(dt);
        for (_, row) in self.row_y.iter_mut() {
            row.step(dt);
        }
        let _ = self.render();

        if self.width.is_settled()
            && self.height.is_settled()
            && self.page_pos.is_settled()
            && self.bubble.is_settled()
            && self.hud_mix.is_settled()
            && self.music_mix.is_settled()
            && self.drop_mix.is_settled()
            && self.row_y.iter().all(|(_, r)| r.is_settled())
        {
            if self.bubble.target == 0.0 {
                self.bubble_flag = None;
            }
            // Slid round the ring past the end: same place, back in 0..count.
            if self.page_pos.value != self.page as f32 {
                self.page_pos = Ease::new(self.page as f32);
            }
            self.animating = false;
        }
    }

    unsafe fn on_dpi_changed(&mut self, dpi: u32) {
        self.dpi_scale = dpi as f32 / 96.0;
        let _ = self.gfx.set_scale(self.dpi_scale);
        let _ = self.resize_surface();
        self.position_window();
        self.refresh_display_info();
        let _ = self.render();
    }

    // ---------------------------------------------------------------- mouse

    fn action_at(&self, x: f32, y: f32) -> Option<Action> {
        self.hits.iter().find(|(r, _)| contains(r, x, y)).map(|(_, a)| a.clone())
    }

    unsafe fn on_mouse_move(&mut self, x: f32, y: f32) {
        let action = self.action_at(x, y);
        if action != self.hover_action {
            self.set_hover_action(action);
            if !self.animating {
                let _ = self.render();
            }
        }
    }

    unsafe fn on_click(&mut self, x: f32, y: f32) {
        let Some(action) = self.action_at(x, y).filter(Action::clickable) else { return };
        match action {
            Action::PagePrev => return self.step_page(-1),
            Action::PageNext => return self.step_page(1),
            Action::Mute => {
                self.audio.toggle_mute();
                self.sound.muted = !self.sound.muted;
                return self.redraw();
            }
            Action::Picker { input } => {
                self.sound = self.audio.state(); // freshest device list
                self.picker = if self.picker == Some(input) { None } else { Some(input) };
                return self.set_expanded(true); // the card grows/shrinks to fit the list
            }
            Action::Device { id, input } => {
                self.audio.set_default(&id, input);
                self.sound = self.audio.state();
                self.picker = None;
                return self.set_expanded(true);
            }
            Action::NetList { bt } => {
                self.close_password();
                self.net_list = if self.net_list == Some(bt) { None } else { Some(bt) };
                if bt {
                    self.bt.refresh();
                } else {
                    self.wifi.refresh_list();
                }
                return self.set_expanded(true);
            }
            Action::Pin => {
                self.pinned = !self.pinned;
                return self.redraw();
            }
            Action::OpenSettings => {
                self.close_password();
                self.picker = None;
                self.net_list = None;
                self.in_settings = true;
                self.settings_tab = TAB_GENERAL;
                self.startup = startup::enabled();
                self.update_stats_wanted();
                self.sync_rows(true);
                self.set_hover_action(None);
                return self.set_expanded(true);
            }
            Action::CloseSettings => {
                self.in_settings = false;
                self.page = 0;
                self.page_pos = Ease::new(0.0);
                self.update_stats_wanted();
                self.set_hover_action(None);
                return self.set_expanded(true);
            }
            Action::TabToggle(page) => {
                self.prefs.toggle(page);
                self.prefs.save();
                return self.redraw();
            }
            Action::TabGrip(_) => return, // dragged, not clicked
            Action::SettingsTab(i) => {
                self.settings_tab = i;
                self.reorder = None;
                self.sync_rows(true);
                self.set_hover_action(None);
                return self.set_expanded(true);
            }
            Action::Startup => {
                self.startup = startup::set(!self.startup);
                return self.redraw();
            }
            Action::AlwaysShow => {
                self.prefs.always_show = !self.prefs.always_show;
                self.prefs.save();
                self.set_bar_mode(self.covered && !self.hovered);
                self.check_covered();
                return self.redraw();
            }
            Action::Speed(k) => {
                self.prefs.speed = k;
                self.prefs.save();
                return self.redraw();
            }
            Action::OpenMode(k) => {
                self.prefs.open_mode = k;
                self.prefs.save();
                self.set_bar_mode(self.covered && !self.hovered);
                return self.redraw();
            }
            Action::DelayBox => {
                if self.delay_edit.is_none() {
                    self.delay_edit = Some(String::new()); // type a fresh value
                    self.take_keyboard();
                }
                return self.redraw();
            }
            Action::Radio { bt } => {
                let on = if bt { self.radio_state.bluetooth } else { self.radio_state.wifi };
                if let Some(on) = on {
                    self.radios.set(bt, !on);
                    self.radio_state = self.radios.state();
                }
                return self.set_expanded(true); // an open list may change size
            }
            Action::WifiRefresh => {
                if self.net_list == Some(true) {
                    self.bt.refresh();
                } else if !self.wifi_state.scanning {
                    self.wifi.scan();
                    self.wifi_state = self.wifi.state();
                }
                return self.redraw();
            }
            Action::Wifi(ssid) => {
                let Some(network) = self.wifi_state.networks.iter().find(|n| n.ssid == ssid).cloned() else { return };
                if network.connected || self.wifi_state.joining.is_some() {
                    return;
                }
                if network.profile.is_some() {
                    self.close_password();
                    self.wifi.connect(&network);
                    self.wifi_state = self.wifi.state();
                    return self.set_expanded(true);
                }
                match network.security {
                    wifi::Security::Open => {
                        self.close_password();
                        self.wifi.join(&network, "");
                        self.wifi_state = self.wifi.state();
                        return self.set_expanded(true);
                    }
                    wifi::Security::Password { .. } => {
                        // New network: type its password right here.
                        if self.pw_ssid.as_deref() == Some(ssid.as_str()) {
                            self.close_password();
                        } else {
                            self.open_password(&ssid);
                        }
                        return self.set_expanded(true);
                    }
                    wifi::Security::Unsupported => return, // the row explains on hover
                }
            }
            Action::PwField => return self.focus_password(),
            Action::PwShow => {
                self.pw_show = !self.pw_show;
                return self.redraw();
            }
            Action::PwJoin => return self.submit_password(),
            Action::ShelfClear => {
                self.shelf.clear();
                self.shelf_art.clear();
                self.shelf_scroll = 0;
                self.switch_page(0);
                return self.set_expanded(self.hovered);
            }
            Action::ShelfRemove(i) => {
                let before = self.page_kind();
                if i < self.shelf.len() {
                    let path = self.shelf.remove(i);
                    self.shelf_art.remove(&path);
                }
                self.after_shelf_change(before);
                return;
            }
            // Tiles are dragged (see on_press / WM_MOUSEMOVE), not clicked.
            Action::ShelfItem(_) => return,
            Action::Bt(name) => {
                let Some(device) = self.bt_state.devices.iter().find(|d| d.name == name).cloned() else { return };
                if self.bt_state.busy.is_some() {
                    return;
                }
                if device.kind == bluetooth::Kind::Audio {
                    self.bt.toggle(&device);
                    self.bt_state = self.bt.state();
                    return self.redraw();
                }
                // Mice, keyboards, phones… reconnect on their own (the row says so on hover).
                return;
            }
            // Sliders act on press (see on_press), not on release.
            Action::Volume { .. } | Action::Brightness { .. } => return,
            _ => {}
        }
        let Some(id) = self.selected.clone() else { return };
        match action {
            Action::Tab(tab) => {
                self.selected = Some(tab);
                // Re-run selection + art upload with the sessions we already have. A copy, not
                // take(): with the list emptied the Music page would vanish for a moment and
                // the notch would jump to the next page.
                let sessions = self.sessions.clone();
                self.apply_snapshot(MediaSnapshot { sessions, system_current: None, notice: None });
                return;
            }
            Action::PlayPause => {
                // Flip the icon straight away; the app confirms a moment later.
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                    s.position = s.position_now();
                    s.sampled_at = Instant::now();
                    s.status = if s.status == Status::Playing { Status::Paused } else { Status::Playing };
                }
                self.media.send(Command::TogglePlayPause(id));
            }
            Action::Next => self.media.send(Command::Next(id)),
            Action::Previous => self.media.send(Command::Previous(id)),
            Action::Shuffle => self.media.send(Command::ToggleShuffle(id)),
            Action::Repeat => self.media.send(Command::CycleRepeat(id)),
            Action::Privacy(_)
            | Action::PagePrev
            | Action::PageNext
            | Action::Mute
            | Action::Picker { .. }
            | Action::Device { .. }
            | Action::Volume { .. }
            | Action::Brightness { .. }
            | Action::NetList { .. }
            | Action::WifiRefresh
            | Action::Wifi(_)
            | Action::Bt(_)
            | Action::Radio { .. }
            | Action::Pin
            | Action::ShelfItem(_)
            | Action::ShelfRemove(_)
            | Action::ShelfClear
            | Action::PwField
            | Action::PwShow
            | Action::PwJoin
            | Action::OpenSettings
            | Action::CloseSettings
            | Action::TabToggle(_)
            | Action::TabGrip(_)
            | Action::SettingsTab(_)
            | Action::Startup
            | Action::AlwaysShow
            | Action::Speed(_)
            | Action::OpenMode(_)
            | Action::DelayBox => {}
            Action::Seek { x0, x1 } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                    let target = (((x - x0) / (x1 - x0)).clamp(0.0, 1.0) as f64) * s.duration;
                    s.position = target;
                    s.sampled_at = Instant::now();
                    self.media.send(Command::Seek(id, target));
                }
            }
        }
        self.update_ticker();
        let _ = self.render();
    }

    unsafe fn redraw(&mut self) {
        if !self.animating {
            let _ = self.render();
        }
    }

    /// Mouse button down: start dragging a slider.
    unsafe fn on_press(&mut self, x: f32, y: f32) {
        let action = self.action_at(x, y);
        if self.delay_edit.is_some() && action != Some(Action::DelayBox) {
            self.end_delay_edit(true); // clicked elsewhere: keep what was typed
        }
        let Some(action) = action else { return };
        if let Action::ShelfItem(i) = action {
            self.shelf_press = Some((i, x, y));
            return;
        }
        if let Action::TabGrip(i) = action {
            let page = self.prefs.order.get(i).copied();
            let top = self.row_y.iter().find(|(p, _)| Some(*p) == page).map_or(0.0, |(_, sp)| sp.value);
            self.grab_offset = y / self.dpi_scale - top;
            self.reorder = Some(i);
            SetCapture(self.hwnd);
            return self.redraw();
        }
        if matches!(action, Action::Volume { .. } | Action::Brightness { .. }) {
            self.dragging = Some(action);
            SetCapture(self.hwnd);
            self.drag_to(x);
        }
    }

    unsafe fn drag_to(&mut self, x: f32) {
        match self.dragging {
            Some(Action::Volume { x0, x1 }) => {
                let v = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
                self.audio.set_volume(v);
                self.sound.volume = v;
                if v > 0.0 {
                    self.sound.muted = false;
                }
            }
            Some(Action::Brightness { x0, x1 }) => {
                let level = (((x - x0) / (x1 - x0)).clamp(0.0, 1.0) * 100.0).round() as u8;
                self.brightness.set(level);
                self.bright = Some(level);
            }
            _ => return,
        }
        self.redraw();
    }

    /// Point every Settings row's animation at its slot (`snap` = jump there now).
    /// The grabbed row is left alone: it follows the pointer.
    fn sync_rows(&mut self, snap: bool) {
        let grabbed = self.reorder.and_then(|i| self.prefs.order.get(i).copied());
        let order = self.prefs.order.clone();
        self.row_y.retain(|(p, _)| order.contains(p));
        for (k, page) in order.iter().enumerate() {
            let slot = SETTINGS_TOP + (k + 1) as f32 * SETTINGS_ROW;
            match self.row_y.iter_mut().find(|(p, _)| p == page) {
                Some((_, spring)) if snap => *spring = Spring::new(slot),
                Some((_, spring)) => {
                    if Some(*page) != grabbed {
                        spring.set_target(slot, ROW);
                    }
                }
                None => self.row_y.push((*page, Spring::new(slot))),
            }
        }
    }

    /// Reordering pages in Settings: the grabbed row follows the pointer; the others make
    /// room for it, gliding into their new places.
    unsafe fn reorder_to(&mut self, y: f32) {
        let Some(from) = self.reorder else { return };
        let Some(page) = self.prefs.order.get(from).copied() else { return };
        let n = self.prefs.order.len();
        let first = SETTINGS_TOP + SETTINGS_ROW;
        let last = SETTINGS_TOP + n as f32 * SETTINGS_ROW;
        let top = (y / self.dpi_scale - self.grab_offset).clamp(first - 8.0, last + 8.0);
        if let Some((_, spring)) = self.row_y.iter_mut().find(|(p, _)| *p == page) {
            *spring = Spring::new(top);
        }
        // Its new slot: wherever its middle is.
        let middle = top + SETTINGS_ROW / 2.0;
        let to = (((middle - first) / SETTINGS_ROW).floor().max(0.0) as usize).min(n.saturating_sub(1));
        if to != from {
            self.prefs.reorder(from, to);
            self.reorder = Some(to);
            self.sync_rows(false);
        }
        self.start_animation();
        self.redraw();
    }

    /// Mouse button up: finish a drag. Returns true if a drag was in progress.
    unsafe fn end_drag(&mut self) -> bool {
        if self.reorder.take().is_some() {
            self.sync_rows(false); // the dropped row settles into its slot
            self.start_animation();
            self.prefs.save();
            let _ = ReleaseCapture();
            self.tracking = false;
            self.track_leave();
            self.redraw();
            return true;
        }
        if self.dragging.take().is_none() {
            return false;
        }
        let _ = ReleaseCapture();
        // If the pointer ended up outside the notch, Windows now tells us so it can close.
        self.tracking = false;
        self.track_leave();
        true
    }

    /// Ask Windows to tell us when the pointer leaves (once per entry).
    unsafe fn track_leave(&mut self) {
        if self.tracking {
            return;
        }
        let mut tme = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        let _ = TrackMouseEvent(&mut tme);
        self.tracking = true;
    }

    /// The pointer arrived: open the card on the page you were on last (Home if it's gone).
    unsafe fn open_card(&mut self) {
        self.set_bar_mode(false);
        self.hovered = true;
        self.hide_hud();
        self.page = self.pages().iter().position(|p| *p == self.prefs.last).unwrap_or(0);
        self.page_pos = Ease::new(self.page as f32);
        if self.page_kind() == Page::Network {
            self.wifi.refresh_list();
            self.bt.refresh();
        }
        self.wheel = 0;
        self.set_expanded(true);
        self.update_stats_wanted();
    }

    /// Close the card (pointer gone, or unpinned with the pointer away).
    unsafe fn close_card(&mut self) {
        self.close_password();
        self.end_delay_edit(true);
        self.in_settings = false;
        self.leaving = false;
        self.hovered = false;
        self.picker = None;
        self.net_list = None;
        self.update_stats_wanted();
        self.set_hover_action(None);
        self.set_expanded(false);
        self.set_bar_mode(self.covered);
    }

    /// Bar mode: the window lets every click through to the app underneath, and we watch
    /// the pointer with a timer instead.
    unsafe fn set_bar_mode(&mut self, on: bool) {
        // Always show: never the thin bar. Click to open: the bar takes clicks (and hovers,
        // through the normal mouse path), so it isn't see-through to the mouse.
        let on = on && !self.prefs.always_show && !self.prefs.click_opens() && !self.fullscreen && !self.music;
        self.dwell_since = None;
        if on == self.click_through {
            return;
        }
        self.click_through = on;
        let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
        let style = if on { style | WS_EX_TRANSPARENT.0 as isize } else { style & !(WS_EX_TRANSPARENT.0 as isize) };
        SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, style);
        if on {
            SetTimer(Some(self.hwnd), BAR_POLL_TIMER, BAR_POLL_MS, None);
        } else {
            let _ = KillTimer(Some(self.hwnd), BAR_POLL_TIMER);
        }
    }

    /// Is the pointer inside the notch's (target) shape right now?
    unsafe fn pointer_over_notch(&self) -> bool {
        let mut p = POINT::default();
        let mut win = RECT::default();
        if GetCursorPos(&mut p).is_err() || GetWindowRect(self.hwnd, &mut win).is_err() {
            return false;
        }
        let s = self.dpi_scale;
        let cx = win.left as f32 + self.px_w as f32 / 2.0;
        let (w, h) = (self.width.target.max(self.width.value) * s, self.height.target.max(self.height.value) * s);
        (p.x as f32) >= cx - w / 2.0 && (p.x as f32) <= cx + w / 2.0 && p.y >= win.top && (p.y as f32) <= win.top as f32 + h
    }

    /// Bar mode: is the pointer on the bar? Open the notch once it has rested there long enough.
    unsafe fn poll_bar(&mut self) {
        let mut p = POINT::default();
        let mut win = RECT::default();
        if GetCursorPos(&mut p).is_err() || GetWindowRect(self.hwnd, &mut win).is_err() {
            return;
        }
        let s = self.dpi_scale;
        let cx = win.left as f32 + self.px_w as f32 / 2.0;
        let half = BAR.0 / 2.0 * s;
        // A little taller than the bar, so flicking the pointer to the top edge finds it.
        let on_bar = (p.x as f32) >= cx - half && (p.x as f32) <= cx + half && p.y >= win.top && (p.y as f32) <= win.top as f32 + (BAR.1 + 4.0) * s;
        match (on_bar, self.dwell_since) {
            (false, _) => self.dwell_since = None,
            (true, None) => self.dwell_since = Some(Instant::now()),
            (true, Some(since)) if since.elapsed() >= Duration::from_millis(self.prefs.hover_ms as u64) => {
                self.open_card();
                self.track_leave();
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ file shelf

    /// Files were dragged onto the notch (true = we'd accept them).
    unsafe fn on_drop_event(&mut self, event: shelf::DropEvent) -> bool {
        match event {
            shelf::DropEvent::Enter => {
                if self.dragging_out || self.click_through {
                    return false; // our own drag, or the click-through bar
                }
                self.dropping = true;
                self.drop_mix.set_target(1.0, EXPAND);
                self.set_expanded(self.hovered);
                true
            }
            shelf::DropEvent::Leave => {
                self.dropping = false;
                self.drop_mix.set_target(0.0, COLLAPSE);
                self.set_expanded(self.hovered);
                false
            }
            shelf::DropEvent::Drop(files) => {
                self.dropping = false;
                self.drop_mix.set_target(0.0, COLLAPSE);
                let before = self.page_kind();
                for path in files {
                    if !self.shelf.contains(&path) {
                        self.thumbs.request(&path);
                        self.shelf.push(path);
                    }
                }
                self.after_shelf_change(before);
                // Show what was dropped: open on the Shelf page (closes when the pointer leaves).
                if let Some(i) = self.pages().iter().position(|p| *p == Page::Shelf) {
                    self.hovered = true;
                    self.hide_hud();
                    self.page = i;
                    self.page_pos = Ease::new(i as f32);
                    self.shelf_scroll = self.shelf.len().saturating_sub(SHELF_TILES);
                }
                self.set_expanded(true);
                self.update_stats_wanted();
                self.tracking = false;
                self.track_leave(); // if the pointer is already elsewhere, this closes it
                true
            }
        }
    }

    /// Files added/removed: keep showing the same page (indexes shift as Shelf comes and goes).
    unsafe fn after_shelf_change(&mut self, before: Page) {
        self.shelf_scroll = self.shelf_scroll.min(self.shelf.len().saturating_sub(SHELF_TILES));
        match self.pages().iter().position(|p| *p == before) {
            Some(i) => {
                if i != self.page {
                    self.page = i;
                    self.page_pos = Ease::new(i as f32);
                }
                self.set_expanded(self.hovered);
            }
            None => self.switch_page(0),
        }
        self.redraw();
    }

    /// Upload finished thumbnails to Direct2D.
    unsafe fn take_thumbnails(&mut self) {
        let t: ID2D1RenderTarget = self.target.clone().into();
        for (path, pixels) in self.thumbs.take() {
            if pixels.len() == (shelf::THUMB * shelf::THUMB * 4) as usize {
                if let Ok(image) = gfx::bitmap(&t, &pixels, shelf::THUMB) {
                    self.shelf_art.insert(path, image);
                }
            }
        }
        self.redraw();
    }

    /// Pointer moved while pressing a shelf tile: far enough means "drag it out".
    /// Returns the files to drag (the caller starts the drag outside the app borrow).
    fn shelf_drag_start(&mut self, x: f32, y: f32) -> Option<Vec<String>> {
        let (i, x0, y0) = self.shelf_press?;
        if (x - x0).abs() + (y - y0).abs() < 6.0 * self.dpi_scale {
            return None;
        }
        self.shelf_press = None;
        let path = self.shelf.get(i)?.clone();
        self.dragging_out = true;
        Some(vec![path])
    }

    /// After a drag out: forget files that no longer exist (e.g. moved), watch for leaving.
    unsafe fn shelf_drag_done(&mut self) {
        self.dragging_out = false;
        let before = self.page_kind();
        let had = self.shelf.len();
        self.shelf.retain(|p| std::path::Path::new(p).exists());
        let art = &mut self.shelf_art;
        art.retain(|p, _| std::path::Path::new(p).exists());
        if self.shelf.len() != had {
            self.after_shelf_change(before);
        }
        self.tracking = false;
        self.track_leave();
    }

    // ------------------------------------------------------------ Wi-Fi password

    unsafe fn open_password(&mut self, ssid: &str) {
        self.pw_ssid = Some(ssid.to_string());
        self.pw_text.clear();
        self.pw_show = false;
        self.focus_password();
    }

    /// Give the password field the keyboard. The notch normally never takes focus, so it
    /// becomes an ordinary active window just while you type.
    unsafe fn focus_password(&mut self) {
        if self.pw_ssid.is_none() {
            return;
        }
        self.pw_focus = true;
        self.take_keyboard();
        self.redraw();
    }

    /// The notch normally never takes focus; it becomes an ordinary active window while you
    /// type in one of its boxes (Wi-Fi password, hover delay), with a blinking caret.
    unsafe fn take_keyboard(&mut self) {
        let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, style & !(WS_EX_NOACTIVATE.0 as isize));
        let _ = SetForegroundWindow(self.hwnd);
        let _ = SetFocus(Some(self.hwnd));
        self.caret_on = true;
        SetTimer(Some(self.hwnd), CARET_TIMER, 530, None);
    }

    /// Typing in a box: stay open, and keys go to it.
    fn typing(&self) -> bool {
        self.pw_focus || self.delay_edit.is_some()
    }

    /// Back to never taking focus. If the pointer is already away, close like a normal leave.
    unsafe fn release_keyboard(&mut self) {
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
    unsafe fn blur_password(&mut self) {
        if !self.pw_focus {
            return;
        }
        self.pw_focus = false;
        self.release_keyboard();
    }

    /// Done typing a hover delay (Enter, clicked elsewhere): keep it if it's a number,
    /// pulled into 0.15–1.5 s. `keep: false` (Esc) leaves the old value.
    unsafe fn end_delay_edit(&mut self, keep: bool) {
        let Some(text) = self.delay_edit.take() else { return };
        if let Some(ms) = settings::parse_delay(&text).filter(|_| keep) {
            self.prefs.hover_ms = ms;
            self.prefs.save();
        }
        self.release_keyboard();
    }

    unsafe fn close_password(&mut self) {
        self.blur_password();
        self.pw_ssid = None;
        self.pw_text.clear(); // don't keep the password around
    }

    unsafe fn submit_password(&mut self) {
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
    unsafe fn on_char(&mut self, ch: u32) {
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

    /// Is a full-screen or maximised app in front, on the notch's monitor? If that changed,
    /// shrink to the thin bar (or grow back).
    /// A full-screen app came to the front (or left): hide the notch and its shadow, and
    /// free their screen-sized pixel buffers while hidden; bring them back after.
    unsafe fn set_fullscreen(&mut self, full: bool) {
        self.fullscreen = full;
        if full {
            self.pinned = false;
            self.close_card();
            self.hide_hud();
            self.set_bar_mode(false);
            let _ = ShowWindow(self.shadow_hwnd, SW_HIDE);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            self.free_surfaces();
        } else {
            let _ = self.resize_surface(); // fails only if Windows is out of memory
            let _ = ShowWindow(self.shadow_hwnd, SW_SHOWNOACTIVATE);
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            stay_on_top(self.hwnd);
            self.redraw();
        }
    }

    /// Swap both big pixel buffers for 1×1 ones (a DC must always hold a bitmap).
    unsafe fn free_surfaces(&mut self) {
        for (dc, bitmap) in [(self.mem_dc, &mut self.bitmap), (self.shadow_dc, &mut self.shadow_bitmap)] {
            let tiny = CreateCompatibleBitmap(dc, 1, 1);
            SelectObject(dc, tiny.into());
            if !bitmap.is_invalid() {
                let _ = DeleteObject((*bitmap).into());
            }
            *bitmap = tiny;
        }
        self.shadow_bits = std::ptr::null_mut();
        self.shadow_key = (-1, -1);
    }

    unsafe fn check_covered(&mut self) {
        let cover = app_covers_screen(self.hwnd);
        let full = cover == Cover::FullScreen && !self.prefs.always_show;
        if full != self.fullscreen {
            self.set_fullscreen(full);
        }
        let covered = cover != Cover::None;
        if covered != self.covered {
            self.covered = covered;
            if !self.hovered {
                self.set_bar_mode(covered);
                self.set_expanded(false);
            }
        }
    }

    /// Wheel over the shelf tiles scrolls them (when there are more than fit).
    unsafe fn wheel_shelf(&mut self, x: f32, y: f32, delta: i32) -> bool {
        if !matches!(self.action_at(x, y), Some(Action::ShelfItem(_) | Action::ShelfRemove(_))) || self.shelf.len() <= SHELF_TILES {
            return false;
        }
        let max = self.shelf.len() - SHELF_TILES;
        self.shelf_scroll = if delta > 0 { self.shelf_scroll.saturating_sub(1) } else { (self.shelf_scroll + 1).min(max) };
        self.redraw();
        true
    }

    /// Mouse wheel over a slider nudges it (2% per notch). Returns true if it did.
    unsafe fn wheel_slider(&mut self, x: f32, y: f32, delta: i32) -> bool {
        let step = delta as f32 / 120.0 * 0.02;
        match self.action_at(x, y) {
            Some(Action::Volume { .. }) => {
                let v = (self.sound.volume + step).clamp(0.0, 1.0);
                self.audio.set_volume(v);
                self.sound.volume = v;
            }
            Some(Action::Brightness { .. }) if self.bright.is_some() => {
                let level = (self.bright.unwrap_or(0) as f32 / 100.0 + step).clamp(0.0, 1.0);
                let level = (level * 100.0).round() as u8;
                self.brightness.set(level);
                self.bright = Some(level);
            }
            _ => return false,
        }
        self.redraw();
        true
    }

    /// Volume or brightness changed from outside the notch (keys, taskbar…): show the
    /// slider in the closed notch for a moment.
    unsafe fn show_hud(&mut self, kind: Hud) {
        if self.hovered {
            return; // the open card already shows it
        }
        self.hud = Some(kind);
        self.hud_kind = kind;
        self.hud_mix.set_target(1.0, EXPAND);
        SetTimer(Some(self.hwnd), HUD_TIMER, HUD_MS, None);
        self.set_expanded(false);
    }

    unsafe fn hide_hud(&mut self) {
        let _ = KillTimer(Some(self.hwnd), HUD_TIMER);
        if self.hud.take().is_some() {
            self.hud_mix.set_target(0.0, COLLAPSE);
            self.set_expanded(self.hovered);
        }
    }

    /// A brightness level arrived from Windows (key press, Action Center, our own slider).
    unsafe fn on_brightness(&mut self, level: u8) {
        let changed = self.bright.is_some_and(|old| old != level);
        self.bright = Some(level);
        if changed && self.dragging.is_none() {
            self.show_hud(Hud::Brightness);
        }
        self.redraw();
    }
}

/// Headphones icon for headphones/headsets/earbuds, a speaker for everything else.
fn device_icon(name: &str) -> Icon {
    let n = name.to_lowercase();
    if ["headphone", "headset", "airpods", "buds", "earphone"].iter().any(|k| n.contains(k)) {
        Icon::Headphones
    } else {
        Icon::Speaker
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.mem_dc);
            let _ = DeleteObject(self.shadow_bitmap.into());
            let _ = DeleteDC(self.shadow_dc);
        }
    }
}

fn mouse_pos(lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
    (x, y)
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // Never take keyboard focus when clicked.
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),

            WM_MOUSEMOVE => {
                let (x, y) = mouse_pos(lparam);
                // Dragging a shelf file out: start Windows' drag outside the app borrow (its
                // drag loop calls back into this window).
                if let Some(paths) = with_app(|app| app.shelf_drag_start(x, y)).flatten() {
                    shelf::drag_out(hwnd, &paths);
                    with_app(|app| app.shelf_drag_done());
                    return LRESULT(0);
                }
                with_app(|app| {
                    if app.dragging.is_some() {
                        app.drag_to(x);
                        return;
                    }
                    if app.reorder.is_some() {
                        app.reorder_to(y);
                        return;
                    }
                    app.track_leave();
                    if app.leaving {
                        // Came back in time: stay open.
                        app.leaving = false;
                        let _ = KillTimer(Some(hwnd), LEAVE_TIMER);
                    }
                    if !app.hovered {
                        if app.click_through {
                            return; // bar mode: the poll timer decides
                        }
                        if !app.prefs.hover_opens() {
                            return; // Open on: Click
                        }
                        // Settings > Motion > Hover delay: open only if it's still here then.
                        if !app.hover_pending {
                            app.hover_pending = true;
                            SetTimer(Some(hwnd), HOVER_TIMER, app.prefs.hover_ms, None);
                        }
                        return;
                    }
                    app.on_mouse_move(x, y);
                });
                LRESULT(0)
            }

            WM_CHAR => {
                with_app(|app| app.on_char(wparam.0 as u32));
                LRESULT(0)
            }

            // The password field lost the keyboard (you clicked another window).
            WM_KILLFOCUS => {
                with_app(|app| {
                    app.blur_password();
                    app.end_delay_edit(true);
                });
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == CARET_TIMER => {
                with_app(|app| {
                    app.caret_on = !app.caret_on;
                    app.redraw();
                });
                LRESULT(0)
            }

            shelf::WM_SHELF_THUMB => {
                with_app(|app| app.take_thumbnails());
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == BAR_POLL_TIMER => {
                with_app(|app| app.poll_bar());
                LRESULT(0)
            }

            WM_MOUSELEAVE => {
                with_app(|app| {
                    app.tracking = false;
                    if app.hover_pending {
                        // Left before the hover delay ran out: don't open.
                        app.hover_pending = false;
                        let _ = KillTimer(Some(hwnd), HOVER_TIMER);
                    }
                    if app.dragging.is_some() || app.reorder.is_some() || !app.hovered {
                        return; // still dragging a slider or a row (the pointer may wander off)
                    }
                    app.set_hover_action(None);
                    app.redraw();
                    if app.pinned || app.typing() {
                        return; // pinned, or typing a password: stay open
                    }
                    // Don't close straight away: a pointer that slips off for a moment
                    // shouldn't snap the notch shut. LEAVE_TIMER closes it.
                    app.leaving = true;
                    SetTimer(Some(hwnd), LEAVE_TIMER, LEAVE_MS, None);
                });
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == HOVER_TIMER => {
                let _ = KillTimer(Some(hwnd), HOVER_TIMER);
                with_app(|app| {
                    let pending = std::mem::take(&mut app.hover_pending);
                    if pending && !app.hovered && !app.click_through && app.pointer_over_notch() {
                        app.open_card();
                        app.track_leave();
                    }
                });
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == LEAVE_TIMER => {
                let _ = KillTimer(Some(hwnd), LEAVE_TIMER);
                with_app(|app| {
                    if !app.leaving || app.pinned || app.typing() {
                        app.leaving = false;
                        return;
                    }
                    if app.pointer_over_notch() {
                        // Still over it (e.g. it just opened from the bar under a pointer that
                        // hasn't moved): keep it open and check again shortly.
                        app.tracking = false;
                        app.track_leave();
                        SetTimer(Some(hwnd), LEAVE_TIMER, LEAVE_MS, None);
                        return;
                    }
                    app.leaving = false;
                    app.close_card();
                });
                LRESULT(0)
            }

            WM_CHECK_COVER => {
                with_app(|app| app.check_covered());
                LRESULT(0)
            }

            // Scroll (wheel or touchpad, vertical or sideways) switches pages.
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
                // Over a slider, the wheel changes its value instead.
                let (sx, sy) = mouse_pos(lparam);
                let mut point = POINT { x: sx as i32, y: sy as i32 };
                let _ = ScreenToClient(hwnd, &mut point);
                let handled = with_app(|app| {
                    (msg == WM_MOUSEWHEEL && app.wheel_slider(point.x as f32, point.y as f32, delta))
                        || app.wheel_shelf(point.x as f32, point.y as f32, delta)
                })
                .unwrap_or(false);
                if !handled {
                    // Wheel down / swipe right = next page.
                    let delta = if msg == WM_MOUSEWHEEL { -delta } else { delta };
                    with_app(|app| app.on_scroll(delta));
                }
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == BUBBLE_TIMER => {
                with_app(|app| app.show_bubble());
                LRESULT(0)
            }

            WM_LBUTTONDOWN => {
                let (x, y) = mouse_pos(lparam);
                with_app(|app| {
                    if !app.hovered && app.prefs.click_opens() {
                        // Open on: Click / Both — open now, no hover delay.
                        app.hover_pending = false;
                        let _ = KillTimer(Some(hwnd), HOVER_TIMER);
                        app.open_card();
                        app.track_leave();
                        return;
                    }
                    app.on_press(x, y);
                });
                LRESULT(0)
            }

            WM_LBUTTONUP => {
                let (x, y) = mouse_pos(lparam);
                with_app(|app| {
                    app.shelf_press = None;
                    if !app.end_drag() {
                        app.on_click(x, y);
                    }
                });
                LRESULT(0)
            }

            audio::WM_AUDIO_UPDATE => {
                with_app(|app| {
                    app.sound = app.audio.state();
                    if lparam.0 == 1 && app.dragging.is_none() {
                        app.show_hud(Hud::Volume);
                    }
                    app.redraw();
                });
                LRESULT(0)
            }

            brightness::WM_BRIGHTNESS_READY => {
                with_app(|app| {
                    if wparam.0 == usize::MAX {
                        app.bright_supported = false;
                    } else if app.bright.is_none() {
                        app.bright = Some(wparam.0.min(100) as u8);
                    }
                    app.redraw();
                });
                LRESULT(0)
            }

            brightness::WM_BRIGHTNESS_CHANGED => {
                with_app(|app| app.on_brightness(wparam.0.min(100) as u8));
                LRESULT(0)
            }

            wifi::WM_WIFI_UPDATE => {
                with_app(|app| {
                    let old_rows = (app.net_rows(), app.password_row_shown());
                    app.wifi_state = app.wifi.state();
                    // Joined: the password row is done.
                    let joined = app.pw_ssid.as_ref().is_some_and(|ssid| app.wifi_state.networks.iter().any(|n| n.connected && &n.ssid == ssid));
                    if joined {
                        app.close_password();
                    }
                    // Join failed: clear the field so the error shows in it ("check the password").
                    let failed = app.wifi_state.joining.is_none()
                        && app.wifi_state.error.as_ref().is_some_and(|(ssid, _)| app.pw_ssid.as_ref() == Some(ssid));
                    if failed && !app.pw_focus {
                        app.pw_text.clear();
                    }
                    if app.hovered && (app.net_rows(), app.password_row_shown()) != old_rows {
                        app.set_expanded(true); // list grew/shrank
                    }
                    app.redraw();
                });
                LRESULT(0)
            }

            bluetooth::WM_BLUETOOTH_UPDATE => {
                with_app(|app| {
                    let old_rows = app.net_rows();
                    app.bt_state = app.bt.state();
                    if app.hovered && app.net_rows() != old_rows {
                        app.set_expanded(true);
                    }
                    app.redraw();
                });
                LRESULT(0)
            }

            radios::WM_RADIO_UPDATE => {
                with_app(|app| {
                    let old_rows = app.net_rows();
                    app.radio_state = app.radios.state();
                    if app.hovered && app.net_rows() != old_rows {
                        app.set_expanded(true);
                    }
                    // Networks/devices appear or vanish with the radio.
                    app.wifi.refresh_list();
                    app.bt.refresh();
                    app.redraw();
                });
                LRESULT(0)
            }

            stats::WM_STATS_UPDATE => {
                with_app(|app| {
                    app.sys = app.stats.get();
                    if app.hovered && app.page_kind() == Page::System {
                        app.redraw();
                    }
                });
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == HUD_TIMER => {
                with_app(|app| app.hide_hud());
                LRESULT(0)
            }

            // Hand cursor over buttons.
            WM_SETCURSOR => {
                let over_button = with_app(|app| app.hover_action.as_ref().is_some_and(Action::clickable)).unwrap_or(false);
                if over_button {
                    if let Ok(cursor) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(Some(cursor));
                        return LRESULT(1);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            WM_MEDIA_UPDATE => {
                with_app(|app| {
                    if let Some(snapshot) = app.media.take_snapshot() {
                        app.apply_snapshot(snapshot);
                    }
                });
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == NOTICE_TIMER => {
                let _ = KillTimer(Some(hwnd), NOTICE_TIMER);
                with_app(|app| {
                    app.notice = None;
                    if !app.animating {
                        let _ = app.render();
                    }
                });
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == CLOCK_TIMER => {
                with_app(|app| {
                    app.clock = sysinfo::clock();
                    app.battery = sysinfo::battery();
                    if !app.animating {
                        let _ = app.render();
                    }
                });
                // Re-aim at the start of the next minute (timers drift slightly).
                SetTimer(Some(hwnd), CLOCK_TIMER, sysinfo::ms_to_next_minute(), None);
                LRESULT(0)
            }

            WM_POWERBROADCAST => {
                if let Some(level) = brightness::parse(wparam, lparam) {
                    with_app(|app| app.on_brightness(level));
                } else if wparam.0 == PBT_APMPOWERSTATUSCHANGE {
                    with_app(|app| {
                        app.battery = sysinfo::battery();
                        if !app.animating {
                            let _ = app.render();
                        }
                    });
                }
                LRESULT(1)
            }

            privacy::WM_PRIVACY_UPDATE => {
                with_app(|app| {
                    app.privacy = wparam.0 as u8;
                    if !app.animating {
                        let _ = app.render();
                    }
                });
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == TOPMOST_TIMER => {
                stay_on_top(hwnd);
                with_app(|app| app.check_covered()); // backup for the window-event hooks
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == TICK_TIMER => {
                with_app(|app| {
                    if !app.animating {
                        let _ = app.render();
                    }
                });
                LRESULT(0)
            }

            WM_DPICHANGED => {
                let dpi = (wparam.0 & 0xFFFF) as u32;
                with_app(|app| app.on_dpi_changed(dpi));
                LRESULT(0)
            }

            // Resolution, refresh rate or monitor layout changed: re-centre and re-sync.
            WM_DISPLAYCHANGE => {
                with_app(|app| {
                    app.position_window();
                    app.refresh_display_info();
                    let _ = app.render();
                });
                LRESULT(0)
            }

            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }

            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::{Paused, Playing, Stopped};

    // F2: the media source you picked stays put.
    #[test]
    fn source_stays_when_another_app_starts_playing() {
        let s = [("spotify", Paused), ("chrome", Playing)];
        assert_eq!(pick_source(Some("spotify"), &s, Some("chrome")).as_deref(), Some("spotify"));
    }

    #[test]
    fn source_ignores_windows_current_changing() {
        let s = [("spotify", Playing), ("chrome", Playing)];
        assert_eq!(pick_source(Some("spotify"), &s, Some("chrome")).as_deref(), Some("spotify"));
    }

    #[test]
    fn second_browser_session_can_stay_selected() {
        let s = [("chrome", Playing), ("chrome#1", Paused)];
        assert_eq!(pick_source(Some("chrome#1"), &s, Some("chrome")).as_deref(), Some("chrome#1"));
    }

    #[test]
    fn source_gone_falls_back_to_playing_windows_current_first() {
        let s = [("vlc", Playing), ("chrome", Playing)];
        assert_eq!(pick_source(Some("spotify"), &s, Some("chrome")).as_deref(), Some("chrome"));
        assert_eq!(pick_source(Some("spotify"), &s, None).as_deref(), Some("vlc"));
    }

    #[test]
    fn stopped_source_counts_as_gone() {
        let s = [("spotify", Stopped), ("chrome", Paused)];
        assert_eq!(pick_source(Some("spotify"), &s, None).as_deref(), Some("chrome"));
    }

    #[test]
    fn nothing_selected_prefers_playing_then_paused() {
        let s = [("spotify", Paused), ("chrome", Paused), ("vlc", Playing)];
        assert_eq!(pick_source(None, &s, Some("chrome")).as_deref(), Some("vlc"));
        let s = [("spotify", Paused), ("chrome", Paused)];
        assert_eq!(pick_source(None, &s, Some("chrome")).as_deref(), Some("chrome"));
        assert_eq!(pick_source(None, &s, None).as_deref(), Some("spotify"));
    }

    #[test]
    fn all_stopped_shows_nothing() {
        assert_eq!(pick_source(Some("spotify"), &[("spotify", Stopped)], Some("spotify")), None);
        assert_eq!(pick_source(None, &[], None), None);
    }

    /// Frames (at 60 Hz) a spring takes to settle at an animation speed.
    fn frames_to_settle(config: spring::SpringConfig, speed: f32) -> u32 {
        let mut sp = Spring::new(0.0);
        sp.set_target(100.0, config);
        let mut frames = 0;
        while !sp.is_settled() && frames < 10_000 {
            sp.step(1.0 / 60.0 * speed);
            frames += 1;
        }
        frames
    }

    // F6: the speed setting makes every animation quicker or slower, same shape.
    #[test]
    fn animation_speed_orders_settle_times() {
        for config in [EXPAND, COLLAPSE] {
            let frames = settings::SPEEDS.map(|(_, k)| frames_to_settle(config, k));
            assert!(frames.windows(2).all(|w| w[0] > w[1]), "each speed is quicker: {frames:?}");
            assert!(frames[0] < 120, "Slow is never sluggish: {frames:?}");
            assert!(frames[6] <= 8, "Wally West is all but instant: {frames:?}");
        }
    }

    #[test]
    fn wally_west_stays_stable() {
        // 10× time per frame mustn't blow the springs up: they still land exactly.
        for config in [EXPAND, COLLAPSE, POP_IN, ROW] {
            let mut sp = Spring::new(0.0);
            sp.set_target(300.0, config);
            for _ in 0..60 {
                sp.step(1.0 / 60.0 * 10.0);
                assert!(sp.value.is_finite() && sp.value.abs() < 600.0);
            }
            assert_eq!(sp.value, 300.0);
        }
        let mut e = Ease::new(0.0);
        e.set_target(5.0);
        for _ in 0..10 {
            e.step(1.0 / 60.0 * 10.0);
        }
        assert_eq!(e.value, 5.0);
    }

    fn sample_notch() -> Outline {
        Outline { cx: 400.0, w: 300.0, h: 200.0, r: 30.0, shoulder: 10.0 }
    }

    // Shadow shader: the distance field matches the notch's shape.
    #[test]
    fn outline_distance_is_exact() {
        let n = sample_notch();
        assert!(n.distance(400.0, 200.0).abs() < 1e-4); // bottom edge
        assert!((n.distance(400.0, 210.0) - 10.0).abs() < 1e-4); // 10 below
        assert!((n.distance(560.0, 50.0) - 10.0).abs() < 1e-4); // 10 right of the side
        assert!((n.distance(400.0, 150.0) + 50.0).abs() < 1e-4); // inside
        // Corner: 10 px out along the diagonal from the corner's centre.
        let (ccx, ccy) = (550.0 - 30.0, 200.0 - 30.0);
        let k = (30.0 + 10.0) / 2f32.sqrt();
        assert!((n.distance(ccx + k, ccy + k) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn shadow_is_thin_smooth_and_hugs_the_edge() {
        for sigma in [3.0, 4.5] {
            let look = ShadowLook { sigma, peak: 0.5, drop: 1.0 };
            let mut last = look.alpha(0.0);
            for tenth in 1..300 {
                let now = look.alpha(tenth as f32 / 10.0);
                assert!(now <= last, "only gets lighter going out");
                assert!(last - now < 0.02, "no hard step ({last} -> {now})");
                last = now;
            }
            // Gone (under one 8-bit step) within ~3.5 sigma of the edge: a few pixels.
            assert!(look.alpha(look.reach() - look.drop) * 255.0 < 0.5);
            assert!(look.reach() < 18.0, "stays close to the edge ({})", look.reach());
        }
    }

    #[test]
    fn painted_shadow_has_no_banding() {
        let n = Outline { cx: 400.0, w: 300.0, h: 600.0, r: 30.0, shoulder: 10.0 };
        let look = ShadowLook { sigma: 3.0, peak: 0.45, drop: 0.0 };
        let (w, h) = (800usize, 700usize);
        let mut px = vec![0u32; w * h];
        paint_shadow(&mut px, w, h, &n, &look);
        // Down the straight right side, each column's average matches the smooth curve to
        // well within one 8-bit step: dithered, not stair-stepped.
        let right = (n.cx + n.w / 2.0) as usize;
        for x in right..right + 10 {
            let mean: f32 = (100..500).map(|y| (px[y * w + x] >> 24) as f32).sum::<f32>() / 400.0;
            let want = look.alpha(x as f32 + 0.5 - (n.cx + n.w / 2.0)) * 255.0;
            assert!((mean - want).abs() < 0.3, "column {x}: {mean} vs {want}");
        }
        // Nothing beyond the reach; nothing deep inside; colour stays black (premultiplied).
        assert!(px.iter().all(|p| p & 0x00FF_FFFF == 0));
        assert_eq!(px[300 * w + right + 20] >> 24, 0);
        assert_eq!(px[300 * w + 400] >> 24, 0);
    }

    #[test]
    fn painted_shadow_fits_the_window() {
        // A notch wider than the window, and a zero-strength shadow, are both safe.
        let mut px = vec![7u32; 50 * 20];
        paint_shadow(&mut px, 50, 20, &Outline { cx: 25.0, w: 80.0, h: 30.0, r: 10.0, shoulder: 8.0 }, &ShadowLook { sigma: 3.0, peak: 0.4, drop: 1.0 });
        paint_shadow(&mut px, 50, 20, &sample_notch(), &ShadowLook { sigma: 3.0, peak: 0.0, drop: 1.0 });
        assert!(px.iter().all(|p| *p == 0));
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

    #[test]
    fn flare_distance_is_exact() {
        // sample_notch: half width 150 (right side at x 550), flare radius 10.
        let n = sample_notch();
        // Facing the flare's curve: straight out from its centre (560, 10).
        let d = n.distance(553.0, 3.0);
        assert!((d - (10.0 - (49.0f32 * 2.0).sqrt())).abs() < 1e-4, "{d}");
        assert!(n.distance(551.0, 1.0) < 0.0, "inside the flare");
        // Beyond the flare: its tip at the screen edge (560, 0) is nearest.
        assert!((n.distance(565.0, 0.0) - 5.0).abs() < 1e-4);
        assert!((n.distance(563.0, 4.0) - 5.0).abs() < 1e-4);
        // Below it: the straight side again.
        assert!((n.distance(556.0, 40.0) - 6.0).abs() < 1e-4);
    }

    #[test]
    fn shadow_wraps_the_flare_smoothly() {
        // The shadow must run round the flared top corners right up to the screen edge (the
        // flares were bare before), with no jumps down the side and no crease out to the side.
        let n = sample_notch();
        let look = ShadowLook { sigma: 2.5, peak: 0.45, drop: 1.0 };
        let (w, h) = (800usize, 300usize);
        let mut px = vec![0u32; w * h];
        paint_shadow(&mut px, w, h, &n, &look);
        let a = |x: usize, y: usize| (px[y * w + x] >> 24) as i32;
        assert!(a(561, 0) > 0, "shadow at the flare's tip, on the screen edge row");
        // Just outside the side, top to bottom: no jumps.
        let side = 550; // pixel centre half a pixel outside the right side
        let column: Vec<i32> = (3..80).map(|y| a(side, y)).collect(); // rows 0-2: under the flare
        assert!(column.windows(2).all(|p| (p[1] - p[0]).abs() <= 12), "{column:?}");
        let want = (look.alpha(0.5) * 255.0) as i32;
        assert!((column[57] - want).abs() <= 1, "full shadow down the side: {} vs {want}", column[57]);
        // Out to the side at the flare's height: fades away, no crease.
        let row: Vec<i32> = (550..590).map(|x| a(x, 10)).collect();
        assert!(row.windows(2).all(|p| p[1] <= p[0] + 2), "only fades going out: {row:?}");
    }

    // Full screen hides the notch; maximised keeps the thin bar.
    #[test]
    fn full_screen_and_maximised_are_told_apart() {
        let monitor = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        let r = |left, top, right, bottom| RECT { left, top, right, bottom };
        // A game or video full screen, or a browser with F11 (often a few px beyond).
        assert_eq!(cover_kind(monitor, monitor, false, false), Cover::FullScreen);
        assert_eq!(cover_kind(r(-8, -8, 1928, 1088), monitor, false, true), Cover::FullScreen);
        // Maximised, taskbar showing: the bar, not hidden.
        assert_eq!(cover_kind(r(-8, -8, 1928, 1040), monitor, true, true), Cover::Maximized);
        // Maximised with an auto-hiding taskbar covers it all, but is still just maximised.
        assert_eq!(cover_kind(r(-8, -8, 1928, 1088), monitor, true, true), Cover::Maximized);
        // A borderless window that Windows calls maximised and covers it all: full screen.
        assert_eq!(cover_kind(monitor, monitor, true, false), Cover::FullScreen);
        // An ordinary window.
        assert_eq!(cover_kind(r(100, 100, 900, 700), monitor, false, true), Cover::None);
        // A second monitor's coordinates.
        let second = RECT { left: 1920, top: 0, right: 4480, bottom: 1440 };
        assert_eq!(cover_kind(second, second, false, false), Cover::FullScreen);
    }

    #[test]
    fn notch_edge_stays_crisp() {
        // On-screen review: the darkest shadow sat right against the edge and blurred the
        // rounded corners. Now the shadow is the notch's shape blurred (as CSS box-shadow):
        // half strength at the edge, darker underneath, lighter outside.
        let look = ShadowLook { sigma: 2.5, peak: 0.6, drop: 0.0 };
        assert!((look.alpha(0.0) - 0.3).abs() < 1e-3);
        assert!(look.alpha(-10.0) > 0.59 && look.alpha(10.0) < 0.001);
        assert!((look.alpha(1.0) + look.alpha(-1.0) - 0.6).abs() < 1e-3, "symmetric about the edge");
    }

    #[test]
    fn no_light_gap_under_the_bottom_edge() {
        // Found in the renders: with the shadow shifted down by more than 2 px, the pixels on
        // the notch's own bottom edge were skipped and the background showed through its
        // anti-aliased rim as a bright line. Every rim pixel must be shaded.
        let n = sample_notch();
        for drop in [0.0, 1.0, 2.75, 4.0] {
            let look = ShadowLook { sigma: 2.0, peak: 0.6, drop };
            let (w, h) = (800usize, 300usize);
            let mut px = vec![0u32; w * h];
            paint_shadow(&mut px, w, h, &n, &look);
            for y in [198usize, 199] {
                let a = px[y * w + 400] >> 24;
                assert!(a as f32 >= look.alpha(0.0) * 255.0 - 1.0, "drop {drop}: rim row {y} = {a}");
            }
        }
    }

    #[test]
    fn every_edge_has_shadow() {
        // On-screen review: too thin, and the flared top corners were bare. Every pixel just
        // outside the outline (sides, round bottom corners, flares up to the screen edge)
        // must get a clear share of the shadow.
        let n = sample_notch();
        let look = ShadowLook { sigma: 3.0, peak: 0.62, drop: 1.0 };
        let (w, h) = (800usize, 300usize);
        let mut px = vec![0u32; w * h];
        paint_shadow(&mut px, w, h, &n, &look);
        let mut checked = 0;
        for y in 0..h {
            for x in 0..w {
                let d = n.distance(x as f32 + 0.5, y as f32 + 0.5);
                if d > 0.25 && d <= 1.25 {
                    let a = (px[y * w + x] >> 24) as f32;
                    assert!(a >= 0.25 * look.peak * 255.0, "bare edge at ({x}, {y}): {a}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 600, "walked the whole outline ({checked})");
    }

    #[test]
    fn flare_shadow_is_even_up_to_the_screen_edge() {
        // On-screen review: shifted down, the shadow pulled away from the flare's curve near
        // the top and pooled into a lump past its tip. Just outside the curve it must match
        // the straight side's shade all the way up.
        let n = sample_notch(); // right side at x 550, flare centre (560, 10), radius 10
        let look = ShadowLook { sigma: 3.0, peak: 0.62, drop: 2.5 };
        let (w, h) = (800usize, 300usize);
        let mut px = vec![0u32; w * h];
        paint_shadow(&mut px, w, h, &n, &look);
        let a = |x: f32, y: f32| (px[y as usize * w + x as usize] >> 24) as f32;
        let side = a(551.0, 60.0);
        for deg in (180..=265).step_by(5) {
            let t = (deg as f32).to_radians();
            let (x, y) = (560.0 + 8.8 * t.cos(), 10.0 + 8.8 * t.sin());
            let v = a(x, y);
            // A concave curve catches a little more shadow than a straight side, but no lump.
            assert!(v >= side * 0.8 && v <= side * 1.6, "{deg}°: {v} vs side {side}");
        }
    }

    #[test]
    fn erfc_is_accurate() {
        for (x, want) in [(0.0, 1.0), (0.5, 0.479_500_1), (1.0, 0.157_299_2), (2.0, 0.004_677_7), (-1.0, 1.842_700_8)] {
            assert!((erfc(x) - want).abs() < 1e-5, "erfc({x}) = {}", erfc(x));
        }
    }

    // Closed now-playing waveform.
    #[test]
    fn waveform_moves_while_playing_and_rests_when_paused() {
        for i in 0..WAVE_BARS {
            let levels: Vec<f32> = (0..300).map(|k| wave_level(i, k as f32 / 60.0, true)).collect();
            let (lo, hi) = levels.iter().fold((1.0f32, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            assert!(lo > 0.1 && hi <= 1.0, "bar {i} stays in range: {lo}..{hi}");
            assert!(hi - lo > 0.2, "bar {i} visibly moves");
            // Smooth at 30 fps: no bar jumps more than a third of its height between frames.
            let frames: Vec<f32> = (0..300).map(|k| wave_level(i, k as f32 / 30.0, true)).collect();
            assert!(frames.windows(2).all(|p| (p[1] - p[0]).abs() < 0.34));
            assert_eq!(wave_level(i, 1.234, false), 0.15, "paused: low and still");
        }
        // Taller in the middle than at the ends, like the iPhone's.
        let peak = |i| (0..600).map(|k| wave_level(i, k as f32 / 60.0, true)).fold(0.0f32, f32::max);
        assert!(peak(2) > peak(0) && peak(2) > peak(WAVE_BARS - 1));
    }
}
