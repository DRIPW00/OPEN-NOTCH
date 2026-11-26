//! Is the camera or microphone in use right now?
//!
//! - Microphone: Windows' audio system lists, for every recording device (real mics and
//!   virtual ones such as NVIDIA Broadcast), the apps recording from it. If any of those
//!   recording sessions is active, the mic is in use. Checked every 1.5 s on a background
//!   thread; each check is a handful of quick system calls, and the notch only redraws
//!   when the answer changes.
//! - Camera: Windows' camera system (Media Foundation's sensor activity monitor) tells us
//!   the moment any app starts or stops streaming from a camera. No checking needed.
//!
//! (Windows' privacy records in the registry looked like an option, but on current Windows
//! they're only written after an app stops using the device, so they can't show live use.)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{implement, w, Interface, Ref, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted to the notch window when the state changes; WPARAM holds the flags below.
pub const WM_PRIVACY_UPDATE: u32 = WM_APP + 3;
pub const CAMERA: u8 = 1;
pub const MIC: u8 = 2;

const MIC_CHECK_EVERY: Duration = Duration::from_millis(1500);

/// What's using the camera and mic, shared with the UI. Posts to the window only when
/// something actually changes.
pub struct Shared {
    flags: AtomicU8,
    users: Mutex<[Vec<String>; 2]>, // [camera apps, mic apps]
    notify: isize,
}

impl Shared {
    /// Apps using the camera (`CAMERA`) or the mic (`MIC`), by name.
    pub fn users(&self, flag: u8) -> Vec<String> {
        let users = self.users.lock().map(|u| u.clone()).unwrap_or_default();
        if flag == CAMERA { users[0].clone() } else { users[1].clone() }
    }

    fn set(&self, flag: u8, mut apps: Vec<String>) {
        apps.sort();
        apps.dedup();
        let index = if flag == CAMERA { 0 } else { 1 };
        let changed_apps = match self.users.lock() {
            Ok(mut users) if users[index] != apps => {
                users[index] = apps.clone();
                true
            }
            _ => false,
        };
        let on = !apps.is_empty();
        let old = if on {
            self.flags.fetch_or(flag, Ordering::SeqCst)
        } else {
            self.flags.fetch_and(!flag, Ordering::SeqCst)
        };
        let new = if on { old | flag } else { old & !flag };
        if new != old || changed_apps {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.notify as *mut _)), WM_PRIVACY_UPDATE, WPARAM(new as usize), LPARAM(0));
            }
        }
    }
}

pub fn start(notify: HWND) -> Arc<Shared> {
    let state = Arc::new(Shared { flags: AtomicU8::new(0), users: Mutex::new([Vec::new(), Vec::new()]), notify: notify.0 as isize });
    let shared = state.clone();
    let _ = std::thread::Builder::new().name("privacy".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        // Camera: event-driven. Kept alive for the life of this thread.
        let _camera = start_camera_monitor(state.clone());

        // Microphone: check the recording sessions.
        let devices: Option<IMMDeviceEnumerator> = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok();
        let mut names = Names::default();
        loop {
            if let Some(devices) = &devices {
                let pids = mic_users(devices);
                state.set(MIC, pids.into_iter().map(|pid| names.get(pid)).collect());
            }
            std::thread::sleep(MIC_CHECK_EVERY);
        }
    });
    shared
}

/// Process ids with an active recording session on any recording device.
unsafe fn mic_users(devices: &IMMDeviceEnumerator) -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(collection) = devices.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else { return pids };
    let count = collection.GetCount().unwrap_or(0);
    for i in 0..count {
        let Ok(device) = collection.Item(i) else { continue };
        let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
        let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
        let n = sessions.GetCount().unwrap_or(0);
        for j in 0..n {
            if let Ok(session) = sessions.GetSession(j) {
                if session.GetState().is_ok_and(|s| s == AudioSessionStateActive) {
                    // Sessions shared by several processes have no single id: count as Windows (0).
                    let pid = session.cast::<IAudioSessionControl2>().and_then(|s| s.GetProcessId()).unwrap_or(0);
                    pids.push(pid);
                }
            }
        }
    }
    pids
}

/// Friendly app names for process ids ("Discord", "NVIDIA Broadcast", "Google Chrome"),
/// from each program's own description. Cached, since apps don't rename themselves.
#[derive(Default)]
struct Names(HashMap<u32, String>);

impl Names {
    fn get(&mut self, pid: u32) -> String {
        self.0.entry(pid).or_insert_with(|| unsafe { app_name(pid) }).clone()
    }
}

unsafe fn app_name(pid: u32) -> String {
    if pid == 0 {
        return "Windows".into();
    }
    let Some(path) = process_path(pid) else { return "Windows".into() }; // protected system process
    let stem = path.rsplit('\\').next().unwrap_or(&path).trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
    if stem.eq_ignore_ascii_case("svchost") || stem.eq_ignore_ascii_case("audiodg") {
        return "Windows".into();
    }
    file_description(&path).filter(|d| !d.trim().is_empty()).unwrap_or(stem)
}

unsafe fn process_path(pid: u32) -> Option<String> {
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut len);
    let _ = CloseHandle(process);
    ok.ok()?;
    Some(String::from_utf16_lossy(&buffer[..len as usize]))
}

/// The "File description" from an .exe's version info, e.g. "Discord".
unsafe fn file_description(path: &str) -> Option<String> {
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let path = PCWSTR(wide.as_ptr());
    let size = GetFileVersionInfoSizeW(path, None);
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    GetFileVersionInfoW(path, None, size, data.as_mut_ptr() as *mut _).ok()?;

    // Which language block to read.
    let mut ptr = std::ptr::null_mut();
    let mut len = 0u32;
    let (lang, codepage) = if VerQueryValueW(data.as_ptr() as *const _, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool() && len >= 4 {
        let pair = std::slice::from_raw_parts(ptr as *const u16, 2);
        (pair[0], pair[1])
    } else {
        (0x0409, 0x04B0) // US English, Unicode
    };
    let key = format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription");
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    if !VerQueryValueW(data.as_ptr() as *const _, PCWSTR(key.as_ptr()), &mut ptr, &mut len).as_bool() || len == 0 {
        return None;
    }
    let text = std::slice::from_raw_parts(ptr as *const u16, len as usize);
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    Some(String::from_utf16_lossy(&text[..end]).trim().to_string())
}

/// Receives camera activity reports from Windows.
#[implement(IMFSensorActivitiesReportCallback)]
struct CameraWatch {
    state: Arc<Shared>,
    names: Mutex<Names>,
}

impl IMFSensorActivitiesReportCallback_Impl for CameraWatch_Impl {
    fn OnActivitiesReport(&self, report: Ref<'_, IMFSensorActivitiesReport>) -> Result<()> {
        let mut apps = Vec::new();
        if let Some(report) = report.as_ref() {
            unsafe {
                let count = report.GetCount().unwrap_or(0);
                for i in 0..count {
                    let Ok(device) = report.GetActivityReport(i) else { continue };
                    let processes = device.GetProcessCount().unwrap_or(0);
                    for p in 0..processes {
                        if let Ok(process) = device.GetProcessActivity(p) {
                            if process.GetStreamingState().is_ok_and(|s| s.as_bool()) {
                                let pid = process.GetProcessId().unwrap_or(0);
                                if let Ok(mut names) = self.names.lock() {
                                    apps.push(names.get(pid));
                                }
                            }
                        }
                    }
                }
            }
        }
        self.state.set(CAMERA, apps);
        Ok(())
    }
}

/// Start Windows' camera activity monitor (Windows 10 2004 and later).
unsafe fn start_camera_monitor(state: Arc<Shared>) -> Option<IMFSensorActivityMonitor> {
    MFStartup(MF_VERSION, MFSTARTUP_LITE).ok()?;
    let callback: IMFSensorActivitiesReportCallback = CameraWatch { state, names: Mutex::new(Names::default()) }.into();
    let monitor = MFCreateSensorActivityMonitor(&callback).ok()?;
    monitor.Start().ok()?;
    Some(monitor)
}
