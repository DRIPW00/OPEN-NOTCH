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
//   Closed, the notch shows the album art and a live equalizer; hovered, it opens into a card
//   with source tabs, the song, a seek bar and controls.
// - While the notch moves we draw one frame per refresh of its monitor (60/120/144/240 Hz...).
//   While music plays we redraw the equalizer at 30 fps (closed) or the progress bar at 4 fps
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

// Hide the console window in release builds (keep it in debug builds for log output).
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
mod stats;
mod settings;
mod sysinfo;
mod tray;
mod uia;
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
const SETTINGS_TOP: f32 = 48.0; // first row
/// Shown under About in Settings.
const ABOUT_VERSION: &str = "R1.00";
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
/// Redraw timer for the equalizer / progress bar while music plays.
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
/// In bar mode (full-screen app) the bar lets clicks through, so we can't see the pointer
/// arrive; instead we check where it is every 100 ms. Resting on the bar for 0.75 s opens it.
const BAR_POLL_TIMER: usize = 8;
const BAR_POLL_MS: u32 = 100;
const DWELL: Duration = Duration::from_millis(750);
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
    /// Frame counter for the fps report printed in debug builds.
    frames: u32,
    anim_start: Instant,
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
    /// The app Windows last reported as its current media app (to notice changes).
    last_system: Option<String>,
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
    /// The pointer left the open notch; it closes when LEAVE_TIMER fires unless it returns.
    leaving: bool,
    /// Windows is watching for the pointer leaving (TrackMouseEvent is armed).
    tracking: bool,
    /// Pinned open: doesn't close when the pointer leaves.
    pinned: bool,
    /// A full-screen or maximised app is in front: shrink to a thin bar.
    covered: bool,
    /// When the pointer arrived on the bar (it opens after DWELL).
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
unsafe fn app_covers_screen(notch: HWND) -> bool {
    let fg = GetForegroundWindow();
    if fg.is_invalid() || fg == notch || !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() {
        return false;
    }
    let mut class = [0u16; 64];
    let len = GetClassNameW(fg, &mut class).max(0) as usize;
    let class = String::from_utf16_lossy(&class[..len]);
    if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return false;
    }
    let monitor = MonitorFromWindow(notch, MONITOR_DEFAULTTOPRIMARY);
    if MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) != monitor {
        return false;
    }
    if IsZoomed(fg).as_bool() {
        return true;
    }
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let mut r = RECT::default();
    if !GetMonitorInfoW(monitor, &mut info).as_bool() || GetWindowRect(fg, &mut r).is_err() {
        return false;
    }
    let m = info.rcMonitor;
    r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
}

fn main() -> Result<()> {
    unsafe {
        // Only one copy at a time: if it's already running, quietly exit.
        // (The handle is never closed, so the lock lasts as long as this process.)
        let _single_instance = CreateMutexW(None, true, w!("Local\\DynamicIslandForWindows"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

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
            frames: 0,
            anim_start: Instant::now(),
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
            last_system: None,
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
            leaving: false,
            tracking: false,
            pinned: false,
            covered: false,
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
            reorder: None,
            row_y: Vec::new(),
            grab_offset: 0.0,
            shadow_hwnd: shadow,
            shadow_dc: CreateCompatibleDC(None),
            shadow_bitmap: HBITMAP::default(),
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
        #[cfg(debug_assertions)]
        println!(
            "monitor: {:.0} Hz, vblank sync: {}",
            self.refresh_hz,
            if self.vblank_output.is_some() { "per-monitor" } else { "compositor (fallback)" }
        );
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

    /// A new picture of what's playing arrived from media.rs.
    ///
    /// Which session the notch shows (and sends commands to): the media app used last.
    /// - When Windows' "current" media app changes (you played/paused/skipped in an app, or
    ///   used the media keys), show that one. Media keys control the same app.
    /// - Clicking a tab picks that app until you use another one.
    unsafe fn apply_snapshot(&mut self, snapshot: MediaSnapshot) {
        let had_media = self.has_media();
        let page_before = self.page_kind();
        if let Some(notice) = snapshot.notice {
            self.notice = Some(notice);
            SetTimer(Some(self.hwnd), NOTICE_TIMER, NOTICE_MS, None);
        }
        self.sessions = snapshot.sessions;

        let exists = |id: &str, sessions: &[Session]| sessions.iter().any(|s| s.id == id);
        if self.selected.as_deref().is_some_and(|id| !exists(id, &self.sessions)) {
            self.selected = None;
        }
        let system = snapshot.system_current.filter(|id| exists(id, &self.sessions));
        let playing: Vec<String> = self
            .sessions
            .iter()
            .filter(|s| s.status == Status::Playing)
            .map(|s| s.id.clone())
            .collect();
        let preferred_playing = system
            .clone()
            .filter(|id| playing.contains(id))
            .or_else(|| playing.first().cloned());

        let system_changed = system.is_some() && system != self.last_system;
        if system.is_some() {
            self.last_system = system.clone();
        }
        if system_changed {
            // You used another media app (or the media keys switched): follow it.
            self.selected = system;
        } else if self.selected.is_none() {
            // Nothing chosen yet: what's playing, else what Windows considers current.
            self.selected = preferred_playing.or(system).or_else(|| {
                self.sessions.iter().find(|s| s.status != Status::Stopped).map(|s| s.id.clone())
            });
        }

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

        #[cfg(debug_assertions)]
        for s in &self.sessions {
            println!(
                "media: [{}] {:?} {} - {} ({}) {}",
                s.app_name,
                s.status,
                s.title,
                s.artist,
                format_time(s.duration),
                if Some(&s.id) == self.selected.as_ref() { "<- shown" } else { "" }
            );
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

    /// Redraw on a timer while music plays: equalizer when closed, progress bar when open.
    unsafe fn update_ticker(&mut self) {
        let playing = self.current().is_some_and(|s| s.status == Status::Playing);
        // ~30 fps while music plays: the equalizer is visible closed and on every open page
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
        } else if self.covered {
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
    /// redrawn when the notch's size changes (not for every equalizer frame).
    unsafe fn render_shadow(&mut self) {
        if self.shadow_hwnd.is_invalid() {
            return;
        }
        let key = ((self.width.value * 2.0).round() as i32, (self.height.value * 2.0).round() as i32);
        if key == self.shadow_key {
            return;
        }
        self.shadow_key = key;
        let rect_px = RECT { left: 0, top: 0, right: self.px_w, bottom: self.px_h };
        if self.target.BindDC(self.shadow_dc, &rect_px).is_err() {
            return;
        }
        let t: ID2D1RenderTarget = self.target.clone().into();
        let brush: ID2D1Brush = self.brush.clone().into();
        let s = self.dpi_scale;
        t.BeginDraw();
        t.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        // No shadow on the thin bar; full shadow once the notch is its normal size.
        let strength = ((self.height.value - BAR.1) / (CLOSED.1 - BAR.1)).clamp(0.0, 1.0);
        if strength > 0.0 {
            // A blur, faked with stacked, ever-larger, very faint rounded rectangles around
            // the notch (the notch window on top covers the middle). It grows with the notch:
            // a tight shadow when closed, a wider, deeper one when open.
            let open = self.openness().clamp(0.0, 1.0);
            let w = self.width.value * s;
            let h = self.height.value * s;
            let cx = self.px_w as f32 / 2.0;
            let corner = (MIN_CORNER + (MAX_CORNER - MIN_CORNER) * open) * s;
            let spread = (10.0 + 18.0 * open) * s; // how far it reaches
            let drop = (2.0 + 4.0 * open) * s; // shifted down: lit from above
            let opacity = (0.05 + 0.02 * open) * strength;
            const LAYERS: usize = 8;
            self.brush.SetColor(&rgb(0x000000, 1.0));
            for i in 1..=LAYERS {
                let e = spread * i as f32 / LAYERS as f32;
                let r = rect(cx - w / 2.0 - e, -e - corner, cx + w / 2.0 + e, h + drop + e);
                self.brush.SetOpacity(opacity);
                t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: corner + e, radiusY: corner + e }, &brush);
            }
        }
        let _ = t.EndDraw(None, None);
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
    unsafe fn draw_idle_closed(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, alpha: f32) {
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

        // Music loaded: a small equalizer, left of the camera/mic dots.
        let dots = self.privacy_colours().len() as f32;
        let eq_right = dots_right - dots * 11.0 * s - if dots > 0.0 { 6.0 * s } else { 0.0 };
        self.draw_equalizer(t, brush, eq_right, h / 2.0, 2.5 * s, 2.0 * s, 12.0 * s, alpha);
    }

    /// The equalizer inside the open card: grows from the closed notch's size to full size
    /// as the notch opens. `right`/`cy` in pixels.
    unsafe fn draw_open_equalizer(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, right: f32, cy: f32, alpha: f32) {
        let s = self.dpi_scale;
        let o = self.openness().clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| (a + (b - a) * o) * s;
        self.draw_equalizer(t, brush, right, cy, lerp(2.5, 3.5), lerp(2.0, 3.0), lerp(12.0, 20.0), alpha);
    }

    /// Three bars in the album's colour, right edge at `right`: bouncing while playing,
    /// flat (small dots) while paused, hidden when nothing is loaded.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_equalizer(&self, t: &ID2D1RenderTarget, brush: &ID2D1Brush, right: f32, cy: f32, bar_w: f32, gap: f32, max_h: f32, alpha: f32) {
        let Some(session) = self.current().filter(|s| s.status != Status::Stopped) else { return };
        let playing = session.status == Status::Playing;
        let accent = session.art.as_ref().map(|a| a.accent).unwrap_or([0.95, 0.65, 0.25]);
        self.brush.SetColor(&D2D1_COLOR_F { r: accent[0], g: accent[1], b: accent[2], a: 1.0 });
        self.brush.SetOpacity(alpha);
        let time = self.started.elapsed().as_secs_f32();
        let bottom = cy + max_h / 2.0;
        let speeds = [7.3, 9.1, 6.2];
        let phases = [0.0, 1.7, 3.1];
        for i in 0..3 {
            // Each bar bounces at its own speed; looks like a live equalizer.
            let level = if playing { 0.3 + 0.7 * (0.5 + 0.5 * (time * speeds[i] + phases[i]).sin()).powf(1.4) } else { 0.0 };
            let bar_h = (max_h * level).max(bar_w);
            let x = right - (3 - i) as f32 * (bar_w + gap) + gap;
            let r = D2D1_ROUNDED_RECT { rect: rect(x, bottom - bar_h, x + bar_w, bottom), radiusX: bar_w / 2.0, radiusY: bar_w / 2.0 };
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

            // Equalizer just left of the battery text.
            let text_w = (self.gfx.measure(&format!("{}%", b.percent), &self.gfx.fonts.battery_big)
                .max(self.gfx.measure(&status, &self.gfx.fonts.battery_status)))
                / s;
            self.draw_open_equalizer(t, brush, at(text_right - text_w - 16.0), y(mid), alpha);
        } else {
            // No battery (desktop PC): the equalizer takes the battery's place.
            self.draw_open_equalizer(t, brush, at(OPEN_IDLE.0 - 24.0), y(mid), alpha);
        }
        hits
    }

    /// Closed: always the Home view, with a small equalizer while music plays.
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
            let offset = (i as f32 - pos) * card_w;
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
            if (i as f32 - pos).abs() < 0.001 {
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
        let home_near = if self.in_settings { 0.0 } else { (1.0 - pos.abs()).clamp(0.0, 1.0) };
        if home_near > 0.01 {
            // Inset past the left page strip, so reaching for it never switches pages.
            let (gx, gy) = (cx - card_w / 2.0 + (SIDE + 22.0) * s, 15.0 * s);
            let gear = Action::OpenSettings;
            let hot = self.hover_action.as_ref() == Some(&gear);
            self.brush.SetColor(&rgb(if hot { 0xFFFFFF } else { 0x636366 }, 1.0));
            self.brush.SetOpacity(alpha * home_near);
            self.gfx.icon(t, Icon::Gear, gx, gy, 13.0 * s, brush);
            if pos.abs() < 0.001 {
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
            let widths: Vec<f32> = (0..count).map(|i| dot + (wide - dot) * (1.0 - (i as f32 - pos).abs()).clamp(0.0, 1.0)).collect();
            let total: f32 = widths.iter().sum::<f32>() + gap * (count - 1) as f32;
            let mut x = cx - total / 2.0;
            let y = h - 10.0 * s;
            for (i, w) in widths.iter().enumerate() {
                let near = (1.0 - (i as f32 - pos).abs()).clamp(0.0, 1.0);
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
        // Equalizer on the right, above the end of the progress bar; text stops short of it.
        self.draw_open_equalizer(t, brush, at(w_box), y(86.0), alpha);
        let text_right = w_box - 36.0;
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
        // Shuffle/repeat stay clickable even when the app doesn't report them: we try, and
        // show a notice if the app refuses.
        let controllable = session.can_play_pause || session.can_next;
        button(x_shuffle, 34.0, Icon::Shuffle, 16.0, Action::Shuffle, controllable, Some(session.shuffle == Some(true)));
        button(x_prev, 36.0, Icon::Previous, 20.0, Action::Previous, session.can_prev, None);
        button(x_next, 36.0, Icon::Next, 20.0, Action::Next, session.can_next, None);
        button(x_repeat, 34.0, Icon::Repeat, 16.0, Action::Repeat, controllable, Some(matches!(session.repeat, Some(Repeat::All | Repeat::One))));
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
        SETTINGS_TOP + (self.prefs.order.len() + 1) as f32 * SETTINGS_ROW + 72.0
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
        set(0x6E6E73, 1.0);
        self.gfx.text(t, "PAGES · DRAG A ROW TO REORDER", &self.gfx.fonts.kind, rect(at(0.0), y(34.0), at(400.0), y(SETTINGS_TOP - 2.0)), brush);

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

        // About
        let top = SETTINGS_TOP + rows.len() as f32 * SETTINGS_ROW + 8.0;
        set(0x6E6E73, 1.0);
        self.gfx.text(t, "ABOUT", &self.gfx.fonts.kind, rect(at(0.0), y(top), at(200.0), y(top + 14.0)), brush);
        set(0xFFFFFF, 1.0);
        self.gfx.text(t, "Dynamic Island for Windows", &self.gfx.fonts.pill, rect(at(0.0), y(top + 16.0), at(w_box), y(top + 36.0)), brush);
        set(0x98989F, 1.0);
        let line = format!("Version {ABOUT_VERSION} · Made by {ABOUT_AUTHOR}");
        self.gfx.text(t, &line, &self.gfx.fonts.time, rect(at(0.0), y(top + 36.0), at(w_box), y(top + 52.0)), brush);
        hits
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
    unsafe fn notch_geometry(&self) -> Result<ID2D1PathGeometry> {
        let s = self.dpi_scale;
        let w = self.width.value.max(1.0) * s;
        let h = self.height.value.max(1.0) * s;
        let cx = self.px_w as f32 / 2.0;
        let (left, right) = (cx - w / 2.0, cx + w / 2.0);

        // Corners grow as the notch opens.
        let t = self.openness().clamp(0.0, 1.0);
        let shoulder = (SHOULDER * s).min(h / 2.0); // smaller on the thin bar
        let r = ((MIN_CORNER + (MAX_CORNER - MIN_CORNER) * t) * s)
            .min((h - shoulder).max(0.0))
            .min(w / 2.0);
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
            self.frames = 0;
            self.anim_start = Instant::now();
        }
    }

    /// Slide to another page (wraps around), resizing the card to fit it.
    unsafe fn switch_page(&mut self, page: usize) {
        let count = self.page_count();
        let page = page % count.max(1);
        if page == self.page && (self.page_pos.target - page as f32).abs() < 0.001 {
            return;
        }
        self.page = page;
        self.picker = None;
        self.net_list = None;
        self.close_password();
        self.update_stats_wanted();
        if self.page_kind() == Page::Network {
            self.wifi.refresh_list();
            self.bt.refresh();
        }
        self.page_pos.set_target(page as f32);
        self.last_page_switch = Instant::now();
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
            let count = self.page_count();
            let next = if self.wheel > 0 { self.page + 1 } else { self.page + count - 1 };
            self.wheel = 0;
            self.switch_page(next);
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

        self.width.step(dt);
        self.height.step(dt);
        self.page_pos.step(dt);
        self.bubble.step(dt);
        self.hud_mix.step(dt);
        self.drop_mix.step(dt);
        for (_, row) in self.row_y.iter_mut() {
            row.step(dt);
        }
        let _ = self.render();
        self.frames += 1;

        if self.width.is_settled()
            && self.height.is_settled()
            && self.page_pos.is_settled()
            && self.bubble.is_settled()
            && self.hud_mix.is_settled()
            && self.drop_mix.is_settled()
            && self.row_y.iter().all(|(_, r)| r.is_settled())
        {
            if self.bubble.target == 0.0 {
                self.bubble_flag = None;
            }
            self.animating = false;
            #[cfg(debug_assertions)]
            {
                let secs = self.anim_start.elapsed().as_secs_f32();
                println!(
                    "animation: {} frames in {:.0} ms = {:.0} fps (monitor: {:.0} Hz)",
                    self.frames,
                    secs * 1000.0,
                    self.frames as f32 / secs,
                    self.refresh_hz
                );
            }
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
            Action::PagePrev => return self.switch_page(self.page + self.page_count() - 1),
            Action::PageNext => return self.switch_page(self.page + 1),
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
                self.sync_rows(true);
                self.set_hover_action(None);
                return self.set_expanded(true);
            }
            Action::CloseSettings => {
                self.in_settings = false;
                self.page = 0;
                self.page_pos = Ease::new(0.0);
                self.set_hover_action(None);
                return self.set_expanded(true);
            }
            Action::TabToggle(page) => {
                self.prefs.toggle(page);
                self.prefs.save();
                return self.redraw();
            }
            Action::TabGrip(_) => return, // dragged, not clicked
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
                // Re-run selection + art upload with the sessions we already have.
                let sessions = std::mem::take(&mut self.sessions);
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
            | Action::TabGrip(_) => {}
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
        let Some(action) = self.action_at(x, y) else { return };
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

    /// The pointer arrived: open the card (always on Home).
    unsafe fn open_card(&mut self) {
        self.set_bar_mode(false);
        self.hovered = true;
        self.hide_hud();
        self.page = 0;
        self.page_pos = Ease::new(0.0);
        self.wheel = 0;
        self.set_expanded(true);
        self.update_stats_wanted();
    }

    /// Close the card (pointer gone, or unpinned with the pointer away).
    unsafe fn close_card(&mut self) {
        self.close_password();
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
            (true, Some(since)) if since.elapsed() >= DWELL => {
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
        let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, style & !(WS_EX_NOACTIVATE.0 as isize));
        let _ = SetForegroundWindow(self.hwnd);
        let _ = SetFocus(Some(self.hwnd));
        self.pw_focus = true;
        self.caret_on = true;
        SetTimer(Some(self.hwnd), CARET_TIMER, 530, None);
        self.redraw();
    }

    /// The field lost the keyboard (clicked elsewhere, Esc, joined).
    unsafe fn blur_password(&mut self) {
        if !self.pw_focus {
            return;
        }
        self.pw_focus = false;
        let _ = KillTimer(Some(self.hwnd), CARET_TIMER);
        let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as isize);
        if !self.tracking && !self.pinned && self.hovered {
            // The pointer is already away: close like a normal leave.
            self.leaving = true;
            SetTimer(Some(self.hwnd), LEAVE_TIMER, LEAVE_MS, None);
        }
        self.redraw();
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

    /// A key typed into the password field.
    unsafe fn on_char(&mut self, ch: u32) {
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
    unsafe fn check_covered(&mut self) {
        let covered = app_covers_screen(self.hwnd);
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
                        app.open_card();
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
                with_app(|app| app.blur_password());
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
                    if app.dragging.is_some() || app.reorder.is_some() || !app.hovered {
                        return; // still dragging a slider or a row (the pointer may wander off)
                    }
                    app.set_hover_action(None);
                    app.redraw();
                    if app.pinned || app.pw_focus {
                        return; // pinned, or typing a password: stay open
                    }
                    // Don't close straight away: a pointer that slips off for a moment
                    // shouldn't snap the notch shut. LEAVE_TIMER closes it.
                    app.leaving = true;
                    SetTimer(Some(hwnd), LEAVE_TIMER, LEAVE_MS, None);
                });
                LRESULT(0)
            }

            WM_TIMER if wparam.0 == LEAVE_TIMER => {
                let _ = KillTimer(Some(hwnd), LEAVE_TIMER);
                with_app(|app| {
                    if !app.leaving || app.pinned || app.pw_focus {
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
                with_app(|app| app.on_press(x, y));
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
