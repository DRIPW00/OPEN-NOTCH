//! Wi-Fi networks for the Network page, through Windows' Native Wifi API.
//!
//! A background thread lists the networks in range (and which one we're on), scans for
//! fresh ones when asked (the refresh button), and connects: to networks Windows already
//! knows directly, and to new ones with the password typed into the notch (we save a
//! Windows network profile for it, like the Wi-Fi menu does). If joining fails, the new
//! profile is removed again and the notch says so.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::NetworkManagement::WiFi::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted to the notch window when the list changes.
pub const WM_WIFI_UPDATE: u32 = WM_APP + 8;

#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    pub ssid: String,
    /// 0–100
    pub signal: u32,
    pub secured: bool,
    pub connected: bool,
    /// Windows has a saved profile (password) for it, so we can connect directly.
    pub profile: Option<String>,
    pub security: Security,
}

/// What joining a new network needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Security {
    /// No password.
    Open,
    /// Password networks (home Wi-Fi): Windows' names for the method and encryption.
    Password { auth: &'static str, encryption: &'static str },
    /// Company/school sign-in (user name + certificate), not handled in the notch.
    Unsupported,
}

impl Security {
    fn from(auth: DOT11_AUTH_ALGORITHM, cipher: DOT11_CIPHER_ALGORITHM, secured: bool) -> Security {
        let encryption = if cipher == DOT11_CIPHER_ALGO_TKIP { "TKIP" } else { "AES" };
        match auth {
            _ if !secured => Security::Open,
            DOT11_AUTH_ALGO_80211_OPEN if cipher == DOT11_CIPHER_ALGO_NONE => Security::Open,
            DOT11_AUTH_ALGO_RSNA_PSK => Security::Password { auth: "WPA2PSK", encryption },
            DOT11_AUTH_ALGO_WPA3_SAE => Security::Password { auth: "WPA3SAE", encryption: "AES" },
            DOT11_AUTH_ALGO_WPA_PSK => Security::Password { auth: "WPAPSK", encryption },
            _ => Security::Unsupported,
        }
    }
}

#[derive(Clone, Default)]
pub struct State {
    /// False if this PC has no Wi-Fi adapter (or it's turned off).
    pub available: bool,
    pub networks: Vec<Network>,
    pub scanning: bool,
    /// SSID we asked to join, until the connection shows up.
    pub joining: Option<String>,
    /// (SSID, message) after a failed join, e.g. a wrong password.
    pub error: Option<(String, String)>,
}

impl State {
    pub fn connected(&self) -> Option<&Network> {
        self.networks.iter().find(|n| n.connected)
    }
}

enum Command {
    List,
    Scan,
    Connect(String),
    /// Save a profile for a new network (with its password), then connect.
    Join(Network, String),
}

pub struct Wifi {
    state: Arc<Mutex<State>>,
    commands: Sender<Command>,
}

impl Wifi {
    pub fn start(notify: HWND) -> Wifi {
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, rx) = channel();
        let shared = state.clone();
        let notify = notify.0 as isize;
        let _ = std::thread::Builder::new().name("wifi".into()).spawn(move || unsafe { run(shared, rx, notify) });
        let wifi = Wifi { state, commands: tx };
        wifi.refresh_list();
        wifi
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Re-read the list (quick, uses Windows' last scan).
    pub fn refresh_list(&self) {
        let _ = self.commands.send(Command::List);
    }

    /// Scan for networks in range (takes a few seconds), then re-read.
    pub fn scan(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.scanning = true;
        }
        let _ = self.commands.send(Command::Scan);
    }

    /// Join a network Windows has a saved profile for.
    pub fn connect(&self, network: &Network) {
        let Some(profile) = network.profile.clone() else { return };
        if let Ok(mut s) = self.state.lock() {
            s.joining = Some(network.ssid.clone());
            s.error = None;
        }
        let _ = self.commands.send(Command::Connect(profile));
    }

    /// Join a new network with this password (empty for open networks).
    pub fn join(&self, network: &Network, password: &str) {
        if let Ok(mut s) = self.state.lock() {
            s.joining = Some(network.ssid.clone());
            s.error = None;
        }
        let _ = self.commands.send(Command::Join(network.clone(), password.to_string()));
    }
}

unsafe fn run(state: Arc<Mutex<State>>, commands: Receiver<Command>, notify: isize) {
    let post = || {
        let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_WIFI_UPDATE, WPARAM(0), LPARAM(0));
    };
    let mut version = 0;
    let mut handle = HANDLE::default();
    if WlanOpenHandle(2, None, &mut version, &mut handle) != 0 {
        post(); // available stays false
        return;
    }
    let update = |state: &Arc<Mutex<State>>| {
        let (available, networks) = match interface(handle) {
            Some(guid) => (true, list(handle, &guid)),
            None => (false, Vec::new()),
        };
        if let Ok(mut s) = state.lock() {
            if s.joining.as_ref().is_some_and(|j| networks.iter().any(|n| n.connected && &n.ssid == j)) {
                s.joining = None;
            }
            s.available = available;
            s.networks = networks;
        }
        post();
    };

    while let Ok(command) = commands.recv() {
        match command {
            Command::List => update(&state),
            Command::Scan => {
                if let Some(guid) = interface(handle) {
                    let _ = WlanScan(handle, &guid, None, None, None);
                    // Scans take up to ~4 s; show what's found along the way.
                    for _ in 0..4 {
                        std::thread::sleep(Duration::from_millis(1000));
                        update(&state);
                    }
                }
                if let Ok(mut s) = state.lock() {
                    s.scanning = false;
                }
                update(&state);
            }
            Command::Join(network, password) => {
                let Some(guid) = interface(handle) else {
                    no_adapter(&state, &network.ssid);
                    update(&state);
                    continue;
                };
                // A profile already saved under this name (e.g. from before the router's security
                // changed) is put back if joining fails, so its saved password isn't lost.
                let name: Vec<u16> = network.ssid.encode_utf16().chain(Some(0)).collect();
                let old = saved_profile(handle, &guid, &name);
                let xml: Vec<u16> = profile_xml(&network, &password).encode_utf16().chain(Some(0)).collect();
                let mut reason = 0;
                if WlanSetProfile(handle, &guid, 0, PCWSTR(xml.as_ptr()), PCWSTR::null(), true, None, &mut reason) != 0 {
                    if let Ok(mut s) = state.lock() {
                        s.joining = None;
                        s.error = Some((network.ssid.clone(), "Couldn't save this network".into()));
                    }
                    update(&state);
                    continue;
                }
                if !connect(handle, &guid, &name) || !wait_connected(&state, &network.ssid, &update, 15) {
                    // Wrong password (or out of range): undo the profile we just made.
                    match old {
                        Some(old) => {
                            let _ = WlanSetProfile(handle, &guid, 0, PCWSTR(old.as_ptr()), PCWSTR::null(), true, None, &mut reason);
                        }
                        None => {
                            let _ = WlanDeleteProfile(handle, &guid, PCWSTR(name.as_ptr()), None);
                        }
                    }
                    if let Ok(mut s) = state.lock() {
                        let message = if password.is_empty() { "Couldn't connect" } else { "Couldn't connect, check the password" };
                        s.error = Some((network.ssid.clone(), message.into()));
                    }
                }
                if let Ok(mut s) = state.lock() {
                    s.joining = None;
                }
                update(&state);
            }
            Command::Connect(profile) => {
                let ssid = state.lock().ok().and_then(|s| s.joining.clone()).unwrap_or_default();
                if let Some(guid) = interface(handle) {
                    let wide: Vec<u16> = profile.encode_utf16().chain(Some(0)).collect();
                    if !connect(handle, &guid, &wide) || !wait_connected(&state, &ssid, &update, 10) {
                        if let Ok(mut s) = state.lock() {
                            s.error = Some((ssid, "Couldn't connect".into()));
                        }
                    }
                } else {
                    no_adapter(&state, &ssid);
                }
                if let Ok(mut s) = state.lock() {
                    s.joining = None;
                }
                update(&state);
            }
        }
    }
}

/// The Wi-Fi adapter went away (disabled, unplugged) before a join could start.
fn no_adapter(state: &Arc<Mutex<State>>, ssid: &str) {
    if let Ok(mut s) = state.lock() {
        s.joining = None;
        s.error = Some((ssid.to_string(), "Wi-Fi adapter not found".into()));
    }
}

/// The XML of a profile Windows has saved under `name` (NUL-terminated UTF-16), NUL-terminated.
unsafe fn saved_profile(handle: HANDLE, guid: &GUID, name: &[u16]) -> Option<Vec<u16>> {
    let mut xml = windows::core::PWSTR::null();
    if WlanGetProfile(handle, guid, PCWSTR(name.as_ptr()), None, &mut xml, None, None) != 0 || xml.is_null() {
        return None;
    }
    let copy = xml.as_wide().iter().copied().chain(Some(0)).collect();
    WlanFreeMemory(xml.0 as *const _);
    Some(copy)
}

/// Ask Windows to connect using a saved profile (`name` is NUL-terminated UTF-16).
unsafe fn connect(handle: HANDLE, guid: &GUID, name: &[u16]) -> bool {
    let params = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: PCWSTR(name.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(),
        pDesiredBssidList: std::ptr::null_mut(),
        dot11BssType: dot11_BSS_type_infrastructure,
        dwFlags: 0,
    };
    WlanConnect(handle, guid, &params, None) == 0
}

/// Re-read the list every second (so the notch stays fresh) until `ssid` shows as connected.
fn wait_connected(state: &Arc<Mutex<State>>, ssid: &str, update: &dyn Fn(&Arc<Mutex<State>>), seconds: u32) -> bool {
    for _ in 0..seconds {
        std::thread::sleep(Duration::from_millis(1000));
        update(state);
        if state.lock().is_ok_and(|s| s.networks.iter().any(|n| n.connected && n.ssid == ssid)) {
            return true;
        }
    }
    false
}

/// A Windows Wi-Fi profile (the same XML the Wi-Fi menu saves) for a new network.
fn profile_xml(network: &Network, password: &str) -> String {
    let ssid = xml_escape(&network.ssid);
    let security = match network.security {
        Security::Password { auth, encryption } => format!(
            "<authEncryption><authentication>{auth}</authentication><encryption>{encryption}</encryption><useOneX>false</useOneX></authEncryption>\
             <sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>",
            xml_escape(password)
        ),
        _ => "<authEncryption><authentication>open</authentication><encryption>none</encryption><useOneX>false</useOneX></authEncryption>".to_string(),
    };
    format!(
        "<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\
         <name>{ssid}</name><SSIDConfig><SSID><name>{ssid}</name></SSID></SSIDConfig>\
         <connectionType>ESS</connectionType><connectionMode>auto</connectionMode>\
         <MSM><security>{security}</security></MSM></WLANProfile>"
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// The first Wi-Fi adapter.
unsafe fn interface(handle: HANDLE) -> Option<GUID> {
    let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
    if WlanEnumInterfaces(handle, None, &mut list) != 0 || list.is_null() {
        return None;
    }
    let count = (*list).dwNumberOfItems as usize;
    let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), count);
    let guid = items.first().map(|i| i.InterfaceGuid);
    WlanFreeMemory(list as *const _);
    guid
}

/// Networks in range: the connected one first, then by signal strength; one entry per name.
unsafe fn list(handle: HANDLE, guid: &GUID) -> Vec<Network> {
    let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
    if WlanGetAvailableNetworkList(handle, guid, 0, None, &mut list) != 0 || list.is_null() {
        return Vec::new();
    }
    let count = (*list).dwNumberOfItems as usize;
    let items = std::slice::from_raw_parts((*list).Network.as_ptr(), count);
    let mut out: Vec<Network> = Vec::new();
    for n in items {
        let len = (n.dot11Ssid.uSSIDLength as usize).min(32);
        let ssid = String::from_utf8_lossy(&n.dot11Ssid.ucSSID[..len]).to_string();
        if ssid.is_empty() {
            continue; // hidden network
        }
        let profile_len = n.strProfileName.iter().position(|&c| c == 0).unwrap_or(0);
        let profile = (n.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0 && profile_len > 0)
            .then(|| String::from_utf16_lossy(&n.strProfileName[..profile_len]));
        let network = Network {
            ssid,
            signal: n.wlanSignalQuality.min(100),
            secured: n.bSecurityEnabled.as_bool(),
            connected: n.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
            profile,
            security: Security::from(n.dot11DefaultAuthAlgorithm, n.dot11DefaultCipherAlgorithm, n.bSecurityEnabled.as_bool()),
        };
        // Windows lists a network once per saved profile plus once without; merge them.
        match out.iter_mut().find(|o| o.ssid == network.ssid) {
            Some(o) => {
                o.connected |= network.connected;
                o.signal = o.signal.max(network.signal);
                if o.profile.is_none() {
                    o.profile = network.profile;
                }
            }
            None => out.push(network),
        }
    }
    WlanFreeMemory(list as *const _);
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)));
    out
}

#[cfg(test)]
#[path = "tests/wifi.rs"]
mod tests;
