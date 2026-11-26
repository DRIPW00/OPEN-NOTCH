//! Turning Wi-Fi and Bluetooth on and off, through Windows' radio controls (the same switches
//! as in Quick Settings). Windows tells us when either changes, wherever it was changed.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::*;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted to the notch window when a radio turns on or off.
pub const WM_RADIO_UPDATE: u32 = WM_APP + 10;

/// None = this PC has no such radio.
#[derive(Clone, Copy, Default)]
pub struct State {
    pub wifi: Option<bool>,
    pub bluetooth: Option<bool>,
}

pub struct Radios {
    state: Arc<Mutex<State>>,
    commands: Sender<(bool, bool)>,
}

impl Radios {
    pub fn start(notify: HWND) -> Radios {
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, rx) = channel();
        let shared = state.clone();
        let notify = notify.0 as isize;
        let _ = std::thread::Builder::new().name("radios".into()).spawn(move || unsafe { run(shared, rx, notify) });
        Radios { state, commands: tx }
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| *s).unwrap_or_default()
    }

    /// Turn Wi-Fi (`bluetooth` = false) or Bluetooth on or off.
    pub fn set(&self, bluetooth: bool, on: bool) {
        if let Ok(mut s) = self.state.lock() {
            // Show the new position straight away; Windows confirms a moment later.
            let slot = if bluetooth { &mut s.bluetooth } else { &mut s.wifi };
            if slot.is_some() {
                *slot = Some(on);
            }
        }
        let _ = self.commands.send((bluetooth, on));
    }
}

fn read(radios: &[(Radio, bool)], state: &Arc<Mutex<State>>, notify: isize) {
    let on = |bt: bool| {
        let mine: Vec<&Radio> = radios.iter().filter(|(_, b)| *b == bt).map(|(r, _)| r).collect();
        (!mine.is_empty()).then(|| mine.iter().any(|r| r.State().is_ok_and(|s| s == RadioState::On)))
    };
    if let Ok(mut s) = state.lock() {
        s.wifi = on(false);
        s.bluetooth = on(true);
    }
    unsafe {
        let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_RADIO_UPDATE, WPARAM(0), LPARAM(0));
    }
}

unsafe fn run(state: Arc<Mutex<State>>, commands: Receiver<(bool, bool)>, notify: isize) {
    let _ = RoInitialize(RO_INIT_MULTITHREADED);
    let _ = Radio::RequestAccessAsync().and_then(|op| op.join());
    let Ok(list) = Radio::GetRadiosAsync().and_then(|op| op.join()) else {
        read(&[], &state, notify);
        return;
    };
    // (radio, is_bluetooth) for the Wi-Fi and Bluetooth radios only.
    let mut radios: Vec<(Radio, bool)> = Vec::new();
    for radio in list {
        match radio.Kind() {
            Ok(RadioKind::WiFi) => radios.push((radio, false)),
            Ok(RadioKind::Bluetooth) => radios.push((radio, true)),
            _ => {}
        }
    }
    let radios = Arc::new(radios);
    for (radio, _) in radios.iter() {
        let (r, s) = (radios.clone(), state.clone());
        let _ = radio.StateChanged(&TypedEventHandler::new(move |_, _| {
            read(&r, &s, notify);
            Ok(())
        }));
    }
    read(&radios, &state, notify);

    while let Ok((bluetooth, on)) = commands.recv() {
        let target = if on { RadioState::On } else { RadioState::Off };
        for (radio, _) in radios.iter().filter(|(_, b)| *b == bluetooth) {
            let _ = radio.SetStateAsync(target).and_then(|op| op.join());
        }
        read(&radios, &state, notify);
    }
}
