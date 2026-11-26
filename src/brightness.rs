//! Screen brightness of the built-in display (laptops), for the Controls page and the HUD.
//!
//! - Noticing changes: the moment the brightness changes (keys, Action Center,
//!   auto-brightness) Windows updates its brightness *setting* and tells us with a
//!   power-setting message; the display driver also raises a WMI event, which a background
//!   thread waits for. (The "current level" message only comes once the screen has faded to
//!   the new level, noticeably late, so it's just a backup.) No polling.
//! - Setting: through WMI's WmiSetBrightness (the standard way for built-in panels), on a
//!   background thread so dragging the slider never stutters.
//!
//! External monitors usually aren't adjustable this way; if there's no built-in display the
//! Controls page shows brightness as unavailable.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use windows::core::{BSTR, GUID, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
use windows::Win32::System::Variant::*;
use windows::Win32::System::Wmi::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, DEVICE_NOTIFY_WINDOW_HANDLE, PBT_POWERSETTINGCHANGE, WM_APP};

/// Posted once the background thread knows whether brightness can be controlled.
/// WPARAM = current level 0–100, or usize::MAX if not supported.
pub const WM_BRIGHTNESS_READY: u32 = WM_APP + 5;
/// Posted the moment the brightness changes. WPARAM = new level 0–100.
pub const WM_BRIGHTNESS_CHANGED: u32 = WM_APP + 6;

/// GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS: "the display's brightness changed, here's the level".
const CURRENT_BRIGHTNESS: GUID = GUID::from_u128(0x8ffee2c6_2d01_46be_adb9_398addc5b4ff);

/// GUID_DEVICE_POWER_POLICY_VIDEO_BRIGHTNESS: the brightness *setting* (what the keys and
/// the slider change), as opposed to the level the panel has faded to.
const SETTING_BRIGHTNESS: GUID = GUID::from_u128(0xaded5e82_b909_4619_9949_f5d71dac0bcb);

/// Ask Windows to tell `hwnd` about brightness changes (arrive as WM_POWERBROADCAST).
pub unsafe fn watch(hwnd: HWND) {
    // The setting changes the moment a key is pressed; the current level only after the
    // screen has faded to it. Listen to both (the second is a backup).
    let _ = RegisterPowerSettingNotification(HANDLE(hwnd.0), &SETTING_BRIGHTNESS, DEVICE_NOTIFY_WINDOW_HANDLE);
    let _ = RegisterPowerSettingNotification(HANDLE(hwnd.0), &CURRENT_BRIGHTNESS, DEVICE_NOTIFY_WINDOW_HANDLE);
}

/// If this WM_POWERBROADCAST is a brightness change, the new level (0–100).
pub unsafe fn parse(wparam: WPARAM, lparam: LPARAM) -> Option<u8> {
    if wparam.0 as u32 != PBT_POWERSETTINGCHANGE || lparam.0 == 0 {
        return None;
    }
    let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
    if (setting.PowerSetting != CURRENT_BRIGHTNESS && setting.PowerSetting != SETTING_BRIGHTNESS) || setting.DataLength < 4 {
        return None;
    }
    let value = std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32);
    Some(value.min(100) as u8)
}

pub struct Brightness {
    target: Arc<AtomicI32>,
    wake: Sender<()>,
}

impl Brightness {
    pub fn start(notify: HWND) -> Brightness {
        let target = Arc::new(AtomicI32::new(-1));
        let (wake, wakeup) = channel::<()>();
        let wanted = target.clone();
        let notify = notify.0 as isize;
        let _ = std::thread::Builder::new().name("brightness".into()).spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let wmi = Wmi::connect();
            let current = wmi.as_ref().and_then(|w| w.current());
            let ready = match current {
                Some(level) => level as usize,
                None => usize::MAX,
            };
            let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_BRIGHTNESS_READY, WPARAM(ready), LPARAM(0));
            let Some(wmi) = wmi.filter(|_| current.is_some()) else { return };
            // Wait for a new target; while dragging, only the latest value is applied.
            while wakeup.recv().is_ok() {
                while wakeup.try_recv().is_ok() {}
                let level = wanted.swap(-1, Ordering::SeqCst);
                if level >= 0 {
                    wmi.set(level as u8);
                }
            }
        });
        // Second thread: waits for the driver's "brightness changed" events.
        let _ = std::thread::Builder::new().name("brightness-events".into()).spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            if let Some(wmi) = Wmi::connect() {
                wmi.watch_events(notify);
            }
        });
        Brightness { target, wake }
    }

    /// Set the brightness (0–100). Returns straight away.
    pub fn set(&self, level: u8) {
        self.target.store(level.min(100) as i32, Ordering::SeqCst);
        let _ = self.wake.send(());
    }
}

struct Wmi {
    services: IWbemServices,
}

impl Wmi {
    unsafe fn connect() -> Option<Wmi> {
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
        let empty = BSTR::new();
        let services = locator
            .ConnectServer(&BSTR::from("ROOT\\WMI"), &empty, &empty, &empty, 0, &empty, None)
            .ok()?;
        CoSetProxyBlanket(
            &services,
            10, // RPC_C_AUTHN_WINNT
            0,  // RPC_C_AUTHZ_NONE
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )
        .ok()?;
        Some(Wmi { services })
    }

    /// The first object a WQL query returns.
    unsafe fn first(&self, query: &str) -> Option<IWbemClassObject> {
        let found = self
            .services
            .ExecQuery(&BSTR::from("WQL"), &BSTR::from(query), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None)
            .ok()?;
        let mut objects = [None];
        let mut returned = 0;
        let _ = found.Next(WBEM_INFINITE, &mut objects, &mut returned);
        objects[0].take()
    }

    /// Current brightness 0–100, or None if there's no adjustable built-in display.
    unsafe fn current(&self) -> Option<u8> {
        let object = self.first("SELECT CurrentBrightness FROM WmiMonitorBrightness WHERE Active=TRUE")?;
        read_level(&object, windows::core::w!("CurrentBrightness"))
    }

    /// Block forever, posting WM_BRIGHTNESS_CHANGED for every brightness-change event.
    unsafe fn watch_events(&self, notify: isize) {
        let events = match self.services.ExecNotificationQuery(
            &BSTR::from("WQL"),
            &BSTR::from("SELECT * FROM WmiMonitorBrightnessEvent"),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        ) {
            Ok(e) => e,
            Err(_) => return,
        };
        loop {
            let mut objects = [None];
            let mut returned = 0;
            let hr = events.Next(WBEM_INFINITE, &mut objects, &mut returned);
            if hr.is_err() || returned == 0 {
                return;
            }
            if let Some(level) = objects[0].take().and_then(|e| read_level(&e, windows::core::w!("Brightness"))) {
                let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_BRIGHTNESS_CHANGED, WPARAM(level as usize), LPARAM(0));
            }
        }
    }

    /// WmiMonitorBrightnessMethods.WmiSetBrightness(Timeout = 0, Brightness = level)
    unsafe fn set(&self, level: u8) {
        let Some(instance) = self.first("SELECT * FROM WmiMonitorBrightnessMethods WHERE Active=TRUE") else { return };
        let mut path = VARIANT::default();
        if instance.Get(windows::core::w!("__PATH"), 0, &mut path, None, None).is_err() {
            return;
        }
        let path = {
            let inner = &path.Anonymous.Anonymous;
            let text = (inner.vt == VT_BSTR).then(|| (*inner.Anonymous.bstrVal).clone());
            let _ = VariantClear(&mut path);
            match text {
                Some(text) => text,
                None => return,
            }
        };

        let mut class = None;
        if self
            .services
            .GetObject(&BSTR::from("WmiMonitorBrightnessMethods"), WBEM_GENERIC_FLAG_TYPE(0), None, Some(&mut class), None)
            .is_err()
        {
            return;
        }
        let Some(class) = class else { return };
        let mut params = None;
        if class.GetMethod(windows::core::w!("WmiSetBrightness"), 0, &mut params, std::ptr::null_mut()).is_err() {
            return;
        }
        let Some(params) = params.and_then(|p| p.SpawnInstance(0).ok()) else { return };
        let _ = params.Put(windows::core::w!("Timeout"), 0, &variant_u32(0), 0);
        let _ = params.Put(windows::core::w!("Brightness"), 0, &variant_u8(level), 0);
        let _ = self.services.ExecMethod(&path, &BSTR::from("WmiSetBrightness"), WBEM_GENERIC_FLAG_TYPE(0), None, &params, None, None);
    }
}

/// A 0–100 number property of a WMI object.
unsafe fn read_level(object: &IWbemClassObject, name: PCWSTR) -> Option<u8> {
    let mut value = VARIANT::default();
    object.Get(name, 0, &mut value, None, None).ok()?;
    let inner = &value.Anonymous.Anonymous;
    let level = match inner.vt {
        VT_UI1 => Some(inner.Anonymous.bVal as u32),
        VT_I4 => Some(inner.Anonymous.lVal as u32),
        VT_UI4 => Some(inner.Anonymous.ulVal),
        _ => None,
    };
    let _ = VariantClear(&mut value);
    level.map(|l| l.min(100) as u8)
}

fn variant_u32(value: u32) -> VARIANT {
    let mut v = VARIANT::default();
    // SAFETY: setting the type tag and the matching union field together.
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = VT_I4; // WMI wants uint32 sent as VT_I4
        inner.Anonymous.lVal = value as i32;
    }
    v
}

fn variant_u8(value: u8) -> VARIANT {
    let mut v = VARIANT::default();
    // SAFETY: as above.
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = VT_UI1;
        inner.Anonymous.bVal = value;
    }
    v
}
