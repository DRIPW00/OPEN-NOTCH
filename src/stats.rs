//! CPU, RAM, GPU and SSD usage for the System page.
//!
//! A background thread samples once a second, but only while the System page is open
//! (`set_wanted(true)`); the rest of the time it sleeps and costs nothing.
//! - CPU: busy time since the last sample (GetSystemTimes).
//! - RAM: GlobalMemoryStatusEx.
//! - GPU: the "GPU Engine" performance counters Task Manager uses (3D engine utilisation),
//!   per graphics card; we show the busiest card.
//! - SSD: free and total space on the Windows drive (C:).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Performance::*;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Posted to the notch window after each sample.
pub const WM_STATS_UPDATE: u32 = WM_APP + 7;

#[derive(Clone, Default)]
pub struct Stats {
    /// 0–1, None until two samples exist.
    pub cpu: Option<f32>,
    pub ram_used: u64,
    pub ram_total: u64,
    /// 0–1 for the busiest graphics card, and its name.
    pub gpu: Option<f32>,
    pub gpu_name: String,
    pub disk_free: u64,
    pub disk_total: u64,
    /// e.g. "Core i7-13650HX"
    pub cpu_name: String,
}

pub struct Monitor {
    stats: Arc<Mutex<Stats>>,
    wanted: Arc<AtomicBool>,
    wake: Sender<()>,
}

impl Monitor {
    pub fn start(notify: HWND) -> Monitor {
        let stats = Arc::new(Mutex::new(Stats::default()));
        let wanted = Arc::new(AtomicBool::new(false));
        let (wake, wakeup) = channel();
        let (s, w) = (stats.clone(), wanted.clone());
        let notify = notify.0 as isize;
        let _ = std::thread::Builder::new().name("stats".into()).spawn(move || unsafe { run(s, w, wakeup, notify) });
        Monitor { stats, wanted, wake }
    }

    pub fn get(&self) -> Stats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Sample while the System page is showing; sleep otherwise.
    pub fn set_wanted(&self, wanted: bool) {
        if self.wanted.swap(wanted, Ordering::SeqCst) != wanted && wanted {
            let _ = self.wake.send(());
        }
    }
}

fn filetime(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

unsafe fn run(stats: Arc<Mutex<Stats>>, wanted: Arc<AtomicBool>, wakeup: Receiver<()>, notify: isize) {
    let mut last_cpu: Option<(u64, u64)> = None; // (idle, total)
    let mut gpu = Gpu::new();
    let cpu_name = cpu_name();
    loop {
        if !wanted.load(Ordering::SeqCst) {
            // Asleep until the page opens.
            if wakeup.recv().is_err() {
                return;
            }
            last_cpu = None; // the old baseline would average over the whole sleep
            continue;
        }

        // CPU
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        let mut cpu = None;
        if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_ok() {
            let (idle, total) = (filetime(idle), filetime(kernel) + filetime(user)); // kernel includes idle
            if let Some((last_idle, last_total)) = last_cpu {
                let (di, dt) = (idle.saturating_sub(last_idle), total.saturating_sub(last_total));
                if dt > 0 {
                    cpu = Some((1.0 - di as f32 / dt as f32).clamp(0.0, 1.0));
                }
            }
            last_cpu = Some((idle, total));
        }

        // RAM
        let mut memory = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        let _ = GlobalMemoryStatusEx(&mut memory);

        // SSD (the Windows drive)
        let (mut free, mut total) = (0u64, 0u64);
        let _ = GetDiskFreeSpaceExW(w!("C:\\"), None, Some(&mut total), Some(&mut free));

        let (gpu_use, gpu_name) = gpu.sample();

        if let Ok(mut s) = stats.lock() {
            if cpu.is_some() {
                s.cpu = cpu;
            }
            s.ram_total = memory.ullTotalPhys;
            s.ram_used = memory.ullTotalPhys.saturating_sub(memory.ullAvailPhys);
            s.disk_free = free;
            s.disk_total = total;
            s.gpu = gpu_use;
            if let Some(name) = gpu_name {
                s.gpu_name = name;
            }
            s.cpu_name = cpu_name.clone();
        }
        let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_STATS_UPDATE, WPARAM(0), LPARAM(0));

        // Next sample in a second (sooner if woken).
        match wakeup.recv_timeout(Duration::from_millis(if cpu.is_none() { 250 } else { 1000 })) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// GPU utilisation from Windows' performance counters, plus card names from DXGI.
struct Gpu {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    ok: bool,
    /// (LUID text as it appears in counter names, card name)
    cards: Vec<(String, String)>,
}

impl Gpu {
    unsafe fn new() -> Gpu {
        let mut query = PDH_HQUERY::default();
        let mut counter = PDH_HCOUNTER::default();
        let mut ok = PdhOpenQueryW(PCWSTR::null(), 0, &mut query) == 0;
        ok = ok && PdhAddEnglishCounterW(query, w!("\\GPU Engine(*engtype_3D)\\Utilization Percentage"), 0, &mut counter) == 0;
        if ok {
            let _ = PdhCollectQueryData(query); // rates need a first sample
        }
        Gpu { query, counter, ok, cards: cards() }
    }

    /// (utilisation of the busiest card 0–1, its name)
    unsafe fn sample(&mut self) -> (Option<f32>, Option<String>) {
        let fallback = self.cards.first().map(|(_, n)| n.clone());
        if !self.ok || PdhCollectQueryData(self.query) != 0 {
            return (None, fallback);
        }
        let (mut size, mut count) = (0u32, 0u32);
        let _ = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
        if size == 0 {
            return (Some(0.0), fallback);
        }
        // Buffer of whole items, big enough for the items and the names after them.
        let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let mut buffer: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> = vec![Default::default(); (size as usize).div_ceil(item) + 1];
        if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(buffer.as_mut_ptr())) != 0 {
            return (None, fallback);
        }
        // Sum per card ("…_luid_0x00000000_0x0000C2F1_phys_0_…"), like Task Manager.
        let mut per_card: Vec<(String, f64)> = Vec::new();
        for entry in &buffer[..count as usize] {
            let Ok(name) = entry.szName.to_string() else { continue };
            if entry.FmtValue.CStatus != 0 {
                continue;
            }
            let value = entry.FmtValue.Anonymous.doubleValue;
            let luid = name.split("_phys").next().and_then(|n| n.split("luid_").nth(1)).unwrap_or("").to_lowercase();
            match per_card.iter_mut().find(|(l, _)| *l == luid) {
                Some((_, total)) => *total += value,
                None => per_card.push((luid, value)),
            }
        }
        let Some((luid, busiest)) = per_card.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)) else {
            return (Some(0.0), fallback);
        };
        let name = self.cards.iter().find(|(l, _)| *l == luid).map(|(_, n)| n.clone()).or(fallback);
        (Some((busiest / 100.0).clamp(0.0, 1.0) as f32), name)
    }
}

/// Graphics cards (not Microsoft's software renderer), biggest video memory first.
unsafe fn cards() -> Vec<(String, String)> {
    let mut out: Vec<(usize, String, String)> = Vec::new();
    let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return Vec::new() };
    let mut i = 0;
    while let Ok(adapter) = factory.EnumAdapters1(i) {
        i += 1;
        let Ok(desc) = adapter.GetDesc1() else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..len]);
        let luid = format!("0x{:08x}_0x{:08x}", desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart).to_lowercase();
        out.push((desc.DedicatedVideoMemory, luid, short_gpu_name(&name)));
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.0));
    out.into_iter().map(|(_, l, n)| (l, n)).collect()
}

/// The processor's name, from the registry, shortened (see `short_cpu_name`).
unsafe fn cpu_name() -> String {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let mut buffer = [0u16; 256];
    let mut size = (buffer.len() * 2) as u32;
    let found = RegGetValueW(
        HKEY_LOCAL_MACHINE,
        w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"),
        w!("ProcessorNameString"),
        RRF_RT_REG_SZ,
        None,
        Some(buffer.as_mut_ptr() as *mut _),
        Some(&mut size),
    );
    if found.is_err() {
        return String::new();
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    short_cpu_name(&String::from_utf16_lossy(&buffer[..len]))
}

/// "13th Gen Intel(R) Core(TM) i7-13650HX" -> "Core i7-13650HX"
/// "AMD Ryzen 7 7840HS w/ Radeon 780M Graphics" -> "Ryzen 7 7840HS"
fn short_cpu_name(name: &str) -> String {
    let mut n = name.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    // Drop everything before the brand line ("13th Gen Intel ", "AMD "). The earliest brand
    // wins: "Ryzen 5 5600X 6-Core Processor" also contains "Core".
    let brands = ["Core", "Ryzen", "Celeron", "Pentium", "Xeon", "Athlon", "Snapdragon", "Apple"];
    if let Some(i) = brands.iter().filter_map(|b| n.find(b)).min() {
        n = n[i..].to_string();
    }
    for cut in [" w/ ", " with ", " CPU", " @", " Processor", " Mobile", " Compute Engine"] {
        if let Some(i) = n.find(cut) {
            n.truncate(i);
        }
    }
    // "Ryzen 9 7950X 16-Core": the core count doesn't fit under the CPU ring.
    if let Some(i) = n.trim_end().rfind(' ').filter(|i| n.trim_end()[i + 1..].ends_with("-Core")) {
        n.truncate(i);
    }
    n.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "NVIDIA GeForce RTX 4060 Laptop GPU" -> "RTX 4060 Laptop GPU"
fn short_gpu_name(name: &str) -> String {
    let mut n = name.trim().to_string();
    for prefix in ["NVIDIA GeForce ", "NVIDIA ", "AMD ", "Intel(R) "] {
        if let Some(rest) = n.strip_prefix(prefix) {
            n = rest.to_string();
            break;
        }
    }
    n.replace("(R)", "").replace("(TM)", "").trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn short_names() {
        assert_eq!(super::short_gpu_name("NVIDIA GeForce RTX 4060 Laptop GPU"), "RTX 4060 Laptop GPU");
        assert_eq!(super::short_gpu_name("Intel(R) Iris(R) Xe Graphics"), "Iris Xe Graphics");
    }

    #[test]
    fn short_cpu_names() {
        assert_eq!(super::short_cpu_name("13th Gen Intel(R) Core(TM) i7-13650HX"), "Core i7-13650HX");
        assert_eq!(super::short_cpu_name("AMD Ryzen 7 7840HS w/ Radeon 780M Graphics"), "Ryzen 7 7840HS");
    }

    // Bug hunt B1: desktop Ryzen names end in "N-Core Processor"; "Core" must not win.
    #[test]
    fn ryzen_core_count_is_not_the_brand() {
        assert_eq!(super::short_cpu_name("AMD Ryzen 5 5600X 6-Core Processor"), "Ryzen 5 5600X");
        assert_eq!(super::short_cpu_name("AMD Ryzen 9 7950X 16-Core Processor"), "Ryzen 9 7950X");
    }

    #[test]
    fn intel_names_with_clock_speed() {
        assert_eq!(super::short_cpu_name("Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz"), "Core i7-8550U");
        assert_eq!(super::short_cpu_name("Intel(R) Core(TM) Ultra 7 155H"), "Core Ultra 7 155H");
    }

    #[test]
    fn unknown_cpu_name_is_kept() {
        assert_eq!(super::short_cpu_name("  Some  Future   Chip "), "Some Future Chip");
        assert_eq!(super::short_cpu_name(""), "");
    }
}
