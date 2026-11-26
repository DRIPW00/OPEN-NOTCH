//! Volume, mute and audio devices, for the Controls page and the volume HUD.
//!
//! A background thread owns everything audio (Windows' audio objects like their own
//! thread). It
//! - listens to the default output's volume: Windows calls us whenever it changes (volume
//!   keys, the taskbar, any app), and we tell the notch so it can show the HUD;
//! - listens for devices being plugged in or out and for the default device changing;
//! - lists the output (speakers, headphones…) and input (microphones) devices;
//! - carries out the notch's commands: set volume, mute, switch device.
//!
//! Nothing here polls: it all waits for Windows to call.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use windows::core::{implement, Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted to the notch window when anything here changes.
/// LPARAM = 1 when the volume or mute changed from outside the notch (show the HUD).
pub const WM_AUDIO_UPDATE: u32 = WM_APP + 4;

/// Marks volume changes the notch itself made, so they don't pop the HUD.
const OUR_CHANGE: GUID = GUID::from_u128(0x5c1d8b2e_6f0a_4e57_9a3d_d1a51e4f7b20);

/// "Friendly name" of an audio device, e.g. "Speakers (Realtek(R) Audio)".
const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// What the notch shows. Read by the UI thread.
#[derive(Clone, Default)]
pub struct State {
    /// 0.0 – 1.0
    pub volume: f32,
    pub muted: bool,
    pub outputs: Vec<Device>,
    pub inputs: Vec<Device>,
    pub default_output: Option<String>,
    pub default_input: Option<String>,
}

impl State {
    pub fn output_name(&self) -> String {
        Self::name_of(&self.outputs, &self.default_output, "No output")
    }
    pub fn input_name(&self) -> String {
        Self::name_of(&self.inputs, &self.default_input, "No microphone")
    }
    fn name_of(list: &[Device], id: &Option<String>, none: &str) -> String {
        id.as_ref()
            .and_then(|id| list.iter().find(|d| &d.id == id))
            .map(|d| d.name.clone())
            .unwrap_or_else(|| none.to_string())
    }
}

enum Command {
    SetVolume(f32),
    ToggleMute,
    /// Make this device the default (for every kind of use: media, calls, system sounds).
    SetDefault(String),
    /// Devices or the default device changed: re-read everything.
    Reload,
}

pub struct Audio {
    state: Arc<Mutex<State>>,
    commands: Sender<Command>,
}

impl Audio {
    pub fn start(notify: HWND) -> Audio {
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, rx) = channel();
        let shared = state.clone();
        let reload = tx.clone();
        let notify = notify.0 as isize;
        let _ = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || unsafe { run(shared, rx, reload, notify) });
        Audio { state, commands: tx }
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Set the volume (0–1). Also updates our copy straight away so the slider follows the
    /// mouse without waiting for Windows.
    pub fn set_volume(&self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        if let Ok(mut s) = self.state.lock() {
            s.volume = volume;
        }
        let _ = self.commands.send(Command::SetVolume(volume));
    }

    pub fn toggle_mute(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.muted = !s.muted;
        }
        let _ = self.commands.send(Command::ToggleMute);
    }

    pub fn set_default(&self, id: &str, input: bool) {
        if let Ok(mut s) = self.state.lock() {
            if input {
                s.default_input = Some(id.to_string());
            } else {
                s.default_output = Some(id.to_string());
            }
        }
        let _ = self.commands.send(Command::SetDefault(id.to_string()));
    }
}

unsafe fn post(notify: isize, show_hud: bool) {
    let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_AUDIO_UPDATE, WPARAM(0), LPARAM(show_hud as isize));
}

/// Windows calls this whenever the default output's volume or mute changes.
#[implement(IAudioEndpointVolumeCallback)]
struct VolumeWatch {
    state: Arc<Mutex<State>>,
    notify: isize,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeWatch_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        let Some(data) = (unsafe { data.as_ref() }) else { return Ok(()) };
        if let Ok(mut s) = self.state.lock() {
            s.volume = data.fMasterVolume;
            s.muted = data.bMuted.as_bool();
        }
        // Changes the notch made itself (dragging the slider) don't pop the HUD.
        unsafe { post(self.notify, data.guidEventContext != OUR_CHANGE) };
        Ok(())
    }
}

/// Windows calls this when devices are plugged in or out, or the default changes.
#[implement(IMMNotificationClient)]
struct DeviceWatch {
    reload: Mutex<Sender<Command>>,
}

impl DeviceWatch_Impl {
    fn reload(&self) {
        if let Ok(tx) = self.reload.lock() {
            let _ = tx.send(Command::Reload);
        }
    }
}

impl IMMNotificationClient_Impl for DeviceWatch_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        self.reload();
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.reload();
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.reload();
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        // Sent once per role; one reload is enough.
        if role == eConsole {
            self.reload();
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

/// The audio thread.
unsafe fn run(state: Arc<Mutex<State>>, commands: Receiver<Command>, reload: Sender<Command>, notify: isize) {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let Ok(devices) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return };

    let device_watch: IMMNotificationClient = DeviceWatch { reload: Mutex::new(reload) }.into();
    let _ = devices.RegisterEndpointNotificationCallback(&device_watch);
    let volume_watch: IAudioEndpointVolumeCallback = VolumeWatch { state: state.clone(), notify }.into();

    // The default output's volume control (changes when the default output changes).
    let mut endpoint: Option<IAudioEndpointVolume> = None;
    let mut bind = |endpoint: &mut Option<IAudioEndpointVolume>| {
        if let Some(old) = endpoint.take() {
            let _ = old.UnregisterControlChangeNotify(&volume_watch);
        }
        *endpoint = devices
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .and_then(|d| d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None))
            .ok();
        if let Some(e) = endpoint.as_ref() {
            let _ = e.RegisterControlChangeNotify(&volume_watch);
        }
        read_all(&devices, endpoint.as_ref(), &state);
        post(notify, false);
    };
    bind(&mut endpoint);

    while let Ok(command) = commands.recv() {
        // Dragging the slider sends lots of volume changes: only the latest one matters.
        let mut command = command;
        while let Ok(next) = commands.try_recv() {
            match (&command, &next) {
                (Command::SetVolume(_), Command::SetVolume(_)) => command = next,
                (Command::Reload, Command::Reload) => {}
                _ => {
                    handle(command, &mut endpoint, &mut bind);
                    command = next;
                }
            }
        }
        handle(command, &mut endpoint, &mut bind);
    }

    unsafe fn handle(command: Command, endpoint: &mut Option<IAudioEndpointVolume>, bind: &mut impl FnMut(&mut Option<IAudioEndpointVolume>)) {
        match command {
            Command::SetVolume(v) => {
                if let Some(e) = endpoint.as_ref() {
                    // Turning the volume up from muted unmutes, like the volume keys.
                    if v > 0.0 && e.GetMute().is_ok_and(|m| m.as_bool()) {
                        let _ = e.SetMute(false, &OUR_CHANGE);
                    }
                    let _ = e.SetMasterVolumeLevelScalar(v, &OUR_CHANGE);
                }
            }
            Command::ToggleMute => {
                if let Some(e) = endpoint.as_ref() {
                    let muted = e.GetMute().is_ok_and(|m| m.as_bool());
                    let _ = e.SetMute(!muted, &OUR_CHANGE);
                }
            }
            Command::SetDefault(id) => {
                set_default_device(&id);
                // Windows confirms through OnDefaultDeviceChanged → Reload.
            }
            Command::Reload => bind(endpoint),
        }
    }
}

/// Re-read volume, mute, both device lists and the defaults.
unsafe fn read_all(devices: &IMMDeviceEnumerator, endpoint: Option<&IAudioEndpointVolume>, state: &Arc<Mutex<State>>) {
    let outputs = list(devices, eRender);
    let inputs = list(devices, eCapture);
    let default_id = |flow| devices.GetDefaultAudioEndpoint(flow, eConsole).ok().and_then(|d| device_id(&d));
    let (default_output, default_input) = (default_id(eRender), default_id(eCapture));
    let volume = endpoint.and_then(|e| e.GetMasterVolumeLevelScalar().ok());
    let muted = endpoint.and_then(|e| e.GetMute().ok()).map(|m| m.as_bool());
    if let Ok(mut s) = state.lock() {
        s.outputs = outputs;
        s.inputs = inputs;
        s.default_output = default_output;
        s.default_input = default_input;
        s.volume = volume.unwrap_or(0.0);
        s.muted = muted.unwrap_or(false);
    }
}

/// Every working device of one kind (outputs or inputs), sorted by name.
unsafe fn list(devices: &IMMDeviceEnumerator, flow: EDataFlow) -> Vec<Device> {
    let mut out = Vec::new();
    let Ok(collection) = devices.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else { return out };
    let count = collection.GetCount().unwrap_or(0);
    for i in 0..count {
        let Ok(device) = collection.Item(i) else { continue };
        let Some(id) = device_id(&device) else { continue };
        let name = friendly_name(&device).unwrap_or_else(|| "Audio device".to_string());
        out.push(Device { id, name });
    }
    out.sort_by_key(|d| d.name.to_lowercase());
    out
}

pub(crate) unsafe fn take_string(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

unsafe fn device_id(device: &IMMDevice) -> Option<String> {
    take_string(device.GetId().ok()?)
}

pub(crate) unsafe fn friendly_name(device: &IMMDevice) -> Option<String> {
    let store = device.OpenPropertyStore(STGM_READ).ok()?;
    let mut value = store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME).ok()?;
    let name = PropVariantToStringAlloc(&value).ok().and_then(|p| take_string(p));
    let _ = PropVariantClear(&mut value);
    name.filter(|n| !n.is_empty())
}

/// Make `id` Windows' default device, for media, calls and system sounds alike.
///
/// Windows has no public function for this; every audio switcher (including Windows' own
/// Sound settings) uses this long-standing internal one, "IPolicyConfig".
unsafe fn set_default_device(id: &str) {
    const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    // Position of SetDefaultEndpoint in the interface's function table
    // (3 from IUnknown, then 10 others before it).
    const SET_DEFAULT_ENDPOINT: usize = 13;
    type SetDefaultEndpoint = unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, ERole) -> windows::core::HRESULT;

    let Ok(unknown) = CoCreateInstance::<_, windows::core::IUnknown>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) else { return };
    let mut raw = std::ptr::null_mut();
    if unknown.query(&IID_POLICY_CONFIG, &mut raw).is_err() || raw.is_null() {
        return;
    }
    let vtable = *(raw as *const *const usize);
    let set: SetDefaultEndpoint = std::mem::transmute(*vtable.add(SET_DEFAULT_ENDPOINT));
    let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
    for role in [eConsole, eMultimedia, eCommunications] {
        let _ = set(raw, PCWSTR(wide.as_ptr()), role);
    }
    // Release the reference `query` added.
    let _ = windows::core::IUnknown::from_raw(raw);
}
