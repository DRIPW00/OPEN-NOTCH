//! Clock, date and battery, formatted the way Windows is set up (12/24-hour, language).

use windows::core::{w, PCWSTR};

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::*;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::GetLocalTime;

pub struct Clock {
    /// "1:08" (no AM/PM)
    pub short: String,
    /// "AM" / "PM", or empty on a 24-hour clock.
    pub marker: String,
    /// "Thursday"
    pub weekday: String,
    /// "THU"
    pub weekday_short: String,
    /// "26"
    pub day: String,
    /// "November"
    pub month: String,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
    pub saver: bool,
    /// Windows' estimate of time left on battery, in seconds (not while charging).
    pub seconds_left: Option<u32>,
}

pub fn now() -> SYSTEMTIME {
    unsafe { GetLocalTime() }
}

/// Milliseconds until the next minute starts (plus a little margin).
pub fn ms_to_next_minute() -> u32 {
    let t = now();
    (60_000 - (t.wSecond as u32 * 1000 + t.wMilliseconds as u32)).max(1) + 30
}

pub fn clock() -> Clock {
    let t = now();
    unsafe {
        let time = |flags: TIME_FORMAT_FLAGS| {
            let mut buffer = [0u16; 64];
            let n = GetTimeFormatEx(PCWSTR::null(), flags, Some(&t), PCWSTR::null(), Some(&mut buffer));
            text(&buffer, n)
        };
        let marker = {
            let mut buffer = [0u16; 16];
            let n = GetTimeFormatEx(PCWSTR::null(), TIME_FORMAT_FLAGS(0), Some(&t), w!("tt"), Some(&mut buffer));
            text(&buffer, n)
        };
        let date = |format: PCWSTR| {
            let mut buffer = [0u16; 64];
            let n = GetDateFormatEx(PCWSTR::null(), ENUM_DATE_FORMATS_FLAGS(0), Some(&t), format, Some(&mut buffer), PCWSTR::null());
            text(&buffer, n)
        };
        // 24-hour clocks have no AM/PM: TIME_NOTIMEMARKER then gives e.g. "13:08".
        Clock {
            short: time(TIME_NOSECONDS | TIME_NOTIMEMARKER).trim().to_string(),
            marker: marker.trim().to_string(),
            weekday: date(w!("dddd")),
            weekday_short: date(w!("ddd")).trim_end_matches('.').to_uppercase(),
            day: date(w!("d")),
            month: date(w!("MMMM")),
        }
    }
}

fn text(buffer: &[u16], written: i32) -> String {
    // `written` includes the terminating null.
    let len = (written.max(1) as usize - 1).min(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

/// None on PCs without a battery.
pub fn battery() -> Option<Battery> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status).ok()? };
    // 128 = no battery, 255 = unknown.
    if status.BatteryFlag & 128 != 0 || status.BatteryFlag == 255 || status.BatteryLifePercent > 100 {
        return None;
    }
    Some(Battery {
        percent: status.BatteryLifePercent,
        charging: status.ACLineStatus == 1,
        saver: status.SystemStatusFlag == 1,
        // u32::MAX means "unknown" (e.g. while charging or just after unplugging).
        seconds_left: (status.ACLineStatus != 1 && status.BatteryLifeTime != u32::MAX && status.BatteryLifeTime > 0)
            .then_some(status.BatteryLifeTime),
    })
}
