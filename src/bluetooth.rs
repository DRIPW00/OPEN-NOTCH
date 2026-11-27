//! Bluetooth devices for the Network page.
//!
//! - Lists the devices paired with this PC and which are connected (Windows' Bluetooth API).
//! - Connects / disconnects Bluetooth audio devices (headphones, earbuds, speakers): their
//!   Windows audio driver accepts a "reconnect" / "disconnect" request. This is the same
//!   trick audio-switcher tools use, since Windows has no public "connect this device" call.
//! - Other devices (mice, keyboards, phones) connect on their own; clicking one does nothing.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::GUID;
use windows::Win32::Devices::Bluetooth::*;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::{IKsControl, KSIDENTIFIER, KSIDENTIFIER_0, KSIDENTIFIER_0_0, KSPROPERTY_TYPE_GET};
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::audio::{friendly_name, take_string};

/// Posted to the notch window when the list changes.
pub const WM_BLUETOOTH_UPDATE: u32 = WM_APP + 9;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Audio,
    Input,
    Phone,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub connected: bool,
    pub kind: Kind,
}

#[derive(Clone, Default)]
pub struct State {
    /// False if Bluetooth is off or missing.
    pub available: bool,
    pub devices: Vec<Device>,
    /// Name of a device we're connecting / disconnecting.
    pub busy: Option<String>,
}

enum Command {
    List,
    Toggle(String, bool),
}

pub struct Bluetooth {
    state: Arc<Mutex<State>>,
    commands: Sender<Command>,
}

impl Bluetooth {
    pub fn start(notify: HWND) -> Bluetooth {
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, rx) = channel();
        let shared = state.clone();
        let notify = notify.0 as isize;
        let _ = std::thread::Builder::new().name("bluetooth".into()).spawn(move || unsafe { run(shared, rx, notify) });
        let bt = Bluetooth { state, commands: tx };
        bt.refresh();
        bt
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn refresh(&self) {
        let _ = self.commands.send(Command::List);
    }

    /// Connect (or disconnect, if connected) an audio device.
    pub fn toggle(&self, device: &Device) {
        if let Ok(mut s) = self.state.lock() {
            s.busy = Some(device.name.clone());
        }
        let _ = self.commands.send(Command::Toggle(device.name.clone(), device.connected));
    }
}

unsafe fn run(state: Arc<Mutex<State>>, commands: Receiver<Command>, notify: isize) {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let post = || {
        let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_BLUETOOTH_UPDATE, WPARAM(0), LPARAM(0));
    };
    let update = |state: &Arc<Mutex<State>>| {
        let devices = paired();
        if let Ok(mut s) = state.lock() {
            s.available = devices.is_some();
            s.devices = devices.unwrap_or_default();
        }
        post();
    };
    while let Ok(command) = commands.recv() {
        match command {
            Command::List => update(&state),
            Command::Toggle(name, connected) => {
                let done = set_connected(&name, !connected);
                // Connecting takes a few seconds; watch for it.
                for _ in 0..(if done { 8 } else { 1 }) {
                    std::thread::sleep(Duration::from_millis(1000));
                    update(&state);
                    let now = state.lock().ok().and_then(|s| s.devices.iter().find(|d| d.name == name).map(|d| d.connected));
                    if now == Some(!connected) {
                        break;
                    }
                }
                if let Ok(mut s) = state.lock() {
                    s.busy = None;
                }
                update(&state);
            }
        }
    }
}

/// Paired devices, connected first. None if Bluetooth is off / missing.
unsafe fn paired() -> Option<Vec<Device>> {
    let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
        dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
        fReturnAuthenticated: true.into(),
        fReturnRemembered: true.into(),
        fReturnUnknown: false.into(),
        fReturnConnected: true.into(),
        fIssueInquiry: false.into(),
        cTimeoutMultiplier: 0,
        hRadio: HANDLE::default(),
    };
    let mut info = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
    let mut out = Vec::new();
    let find = match BluetoothFindFirstDevice(&params, &mut info) {
        Ok(find) => find,
        // No devices is fine; no radio means Bluetooth is off or missing.
        Err(e) if e.code() == ERROR_NO_MORE_ITEMS.to_hresult() => return Some(out),
        Err(_) => return None,
    };
    loop {
        let len = info.szName.iter().position(|&c| c == 0).unwrap_or(info.szName.len());
        let name = String::from_utf16_lossy(&info.szName[..len]);
        if !name.is_empty() && !out.iter().any(|d: &Device| d.name == name) {
            out.push(Device { name, connected: info.fConnected.as_bool(), kind: kind(info.ulClassofDevice) });
        }
        info = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
        if BluetoothFindNextDevice(find, &mut info).is_err() {
            break;
        }
    }
    let _ = BluetoothFindDeviceClose(find);
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Some(out)
}

/// The "major device class" in a Bluetooth class-of-device number.
fn kind(class: u32) -> Kind {
    match (class >> 8) & 0x1F {
        0x04 => Kind::Audio,
        0x05 => Kind::Input,
        0x02 => Kind::Phone,
        _ => Kind::Other,
    }
}

/// Ask the Bluetooth audio driver(s) for `name` to connect or disconnect.
/// Returns false if no Bluetooth audio driver matched.
unsafe fn set_connected(name: &str, connect: bool) -> bool {
    // KSPROPSETID_BtAudio: KSPROPERTY_ONESHOT_RECONNECT = 0, KSPROPERTY_ONESHOT_DISCONNECT = 1
    const BT_AUDIO: GUID = GUID::from_u128(0x7fa06c40_b8f6_4c7e_8556_e8c33a12e54d);
    let Ok(devices) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return false };
    let mut done = false;
    let mut seen: Vec<String> = Vec::new();
    for flow in [eRender, eCapture] {
        let Ok(endpoints) = devices.EnumAudioEndpoints(flow, DEVICE_STATE(DEVICE_STATEMASK_ALL)) else { continue };
        for i in 0..endpoints.GetCount().unwrap_or(0) {
            // Endpoint -> the driver ("KS filter") behind it.
            let Ok(endpoint) = endpoints.Item(i) else { continue };
            let Ok(topology) = endpoint.Activate::<IDeviceTopology>(CLSCTX_ALL, None) else { continue };
            let Ok(connector) = topology.GetConnector(0) else { continue };
            let Some(filter_id) = connector.GetDeviceIdConnectedTo().ok().and_then(|p| take_string(p)) else { continue };
            if seen.contains(&filter_id) {
                continue;
            }
            seen.push(filter_id.clone());
            let wide: Vec<u16> = filter_id.encode_utf16().chain(Some(0)).collect();
            let Ok(filter) = devices.GetDevice(windows::core::PCWSTR(wide.as_ptr())) else { continue };
            let Some(filter_name) = friendly_name(&filter) else { continue };
            if !names_match(&filter_name, name) {
                continue;
            }
            let Ok(control) = filter.Activate::<IKsControl>(CLSCTX_ALL, None) else { continue };
            let property = KSIDENTIFIER {
                Anonymous: KSIDENTIFIER_0 {
                    Anonymous: KSIDENTIFIER_0_0 { Set: BT_AUDIO, Id: if connect { 0 } else { 1 }, Flags: KSPROPERTY_TYPE_GET },
                },
            };
            let mut returned = 0;
            if control
                .KsProperty(&property, std::mem::size_of::<KSIDENTIFIER>() as u32, std::ptr::null_mut(), 0, &mut returned)
                .is_ok()
            {
                done = true;
            }
        }
    }
    done
}

/// "AirPods Pro" matches drivers named "AirPods Pro", "AirPods Pro Stereo", "AirPods Pro Hands-Free",
/// but not "AirPods Pro 2 Stereo" (a different device).
fn names_match(filter: &str, device: &str) -> bool {
    let (f, d) = (filter.to_lowercase(), device.to_lowercase());
    f == d || f.strip_prefix(&d).and_then(|rest| rest.strip_prefix(' ')).is_some_and(|rest| rest.starts_with("stereo") || rest.starts_with("hands-free"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn matching() {
        assert!(super::names_match("AirPods Pro Stereo", "AirPods Pro"));
        assert!(super::names_match("AirPods Pro", "AirPods Pro"));
        assert!(!super::names_match("Speakers", "AirPods Pro"));
        assert!(super::names_match("AirPods Pro Hands-Free", "AirPods Pro"));
        assert!(!super::names_match("JBL Flip 5 Stereo", "JBL Flip"));
        assert!(!super::names_match("AirPods Pro Hands-Free", "AirPods"));
        assert!(super::names_match("airpods pro stereo", "AirPods Pro")); // case doesn't matter
    }

    #[test]
    fn device_kinds_from_class() {
        use super::{kind, Kind};
        assert!(matches!(kind(0x24_0404), Kind::Audio)); // headphones
        assert!(matches!(kind(0x00_0540), Kind::Input)); // keyboard
        assert!(matches!(kind(0x5A_020C), Kind::Phone));
        assert!(matches!(kind(0), Kind::Other));
    }
}
