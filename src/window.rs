//! The notch's windows: startup, the message loop, the window procedure, staying on top, and full-screen detection.

use super::*;

thread_local! {
    pub(crate) static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    pub(crate) static NOTCH_HWND: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    pub(crate) static SHADOW_HWND: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

/// Put the notch back at the very top of the always-on-top windows, without moving,
/// resizing or activating it.
pub(crate) unsafe fn stay_on_top(hwnd: HWND) {
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
pub(crate) extern "system" fn shadow_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Called by Windows whenever another app comes to the front.
pub(crate) unsafe extern "system" fn on_foreground_changed(
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
pub(crate) unsafe extern "system" fn on_window_changed(
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
pub(crate) unsafe fn app_covers_screen(notch: HWND) -> Cover {
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

pub(crate) fn run() -> Result<()> {
    unsafe {
        // Only one copy at a time: if it's already running, quietly exit.
        // (The handle is never closed, so the lock lasts as long as this process.)
        let _single_instance = CreateMutexW(None, true, w!("Local\\OpenNotch"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        // Up to R1.03 the app was "Dynamic Island", with another lock name. Close a copy of
        // it that's still running (e.g. started with Windows), or the two notches stack.
        if let Ok(old) = FindWindowW(w!("DynamicIslandWindow"), PCWSTR::null()) {
            let _ = PostMessageW(Some(old), WM_CLOSE, WPARAM(0), LPARAM(0));
        }

        // "Launch at startup" keeps working if this folder was moved.
        startup::refresh_path();

        // Crisp rendering on high-DPI screens, per monitor.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("OpenNotchWindow");
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
            w!("OpenNotch"),
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
        let shadow_class = w!("OpenNotchShadow");
        let shadow_wc = WNDCLASSW { lpfnWndProc: Some(shadow_proc), hInstance: instance, lpszClassName: shadow_class, ..Default::default() };
        RegisterClassW(&shadow_wc);
        let shadow = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            shadow_class,
            w!("OpenNotch shadow"),
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
pub(crate) unsafe fn run_message_loop() {
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
pub(crate) unsafe fn wait_for_next_refresh(frame_start: Instant) {
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
pub(crate) unsafe fn find_dxgi_output(monitor: HMONITOR) -> Option<IDXGIOutput> {
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
pub(crate) unsafe fn monitor_refresh_hz(monitor: HMONITOR) -> Option<f32> {
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
pub(crate) fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

/// The Direct2D drawing surface. We draw on the CPU ("software"): for one small shape
/// it's faster than drawing on the GPU and copying the result back every frame, it works
/// the same on every machine, and it can't be lost when the graphics driver resets.
pub(crate) unsafe fn create_render_target(
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

pub(crate) fn mouse_pos(lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
    (x, y)
}

pub(crate) extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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

            WM_TIMER if wparam.0 == PAUSED_TIMER => {
                let _ = KillTimer(Some(hwnd), PAUSED_TIMER);
                with_app(|app| {
                    app.update_ticker();
                    app.redraw();
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
