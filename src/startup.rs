//! "Launch at startup": a value under HKCU\...\Run, the same place Task Manager's Startup
//! apps list reads. The registry is the only record of it, so the switch always shows the
//! real state, including when it was turned off in Task Manager (StartupApproved).

use windows::core::{w, PCWSTR};
use windows::Win32::System::Registry::*;

const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const APPROVED: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");
const NAME: PCWSTR = w!("Dynamic Island");

/// Starts with Windows: our Run value exists and Task Manager hasn't disabled it.
pub fn enabled() -> bool {
    unsafe { read(RUN).is_some() && read(APPROVED).is_none_or(|data| approved(&data)) }
}

/// Turn it on or off. Returns the state afterwards (what the switch should show).
pub fn set(on: bool) -> bool {
    unsafe {
        if on {
            if let Some(exe) = exe() {
                write(&command(&exe));
                // Clear a "disabled" left by Task Manager, or the Run value would be ignored.
                delete(APPROVED);
            }
        } else {
            delete(RUN);
        }
    }
    enabled()
}

/// If it's on but points at an exe that no longer exists (the folder was moved), point it
/// here. A second copy run from elsewhere doesn't take it over.
pub fn refresh_path() {
    let Some(exe) = exe() else { return };
    unsafe {
        if let Some(data) = read(RUN) {
            if stale(&text(&data)) {
                write(&command(&exe));
            }
        }
    }
}

/// The Run value names an exe that isn't there any more.
fn stale(run: &str) -> bool {
    !std::path::Path::new(run.trim().trim_matches('"')).exists()
}

fn exe() -> Option<String> {
    std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned())
}

/// The Run value: the exe's path in quotes (paths have spaces: "Dynamic Island.exe").
fn command(exe: &str) -> String {
    format!("\"{exe}\"")
}

/// StartupApproved data: the first byte's low bit set means "disabled in Task Manager".
fn approved(data: &[u8]) -> bool {
    data.first().is_none_or(|b| b & 1 == 0)
}

/// UTF-16LE registry string bytes -> String (without the trailing NUL).
fn text(data: &[u8]) -> String {
    let wide: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&wide).trim_end_matches('\0').to_string()
}

unsafe fn read(key: PCWSTR) -> Option<Vec<u8>> {
    let mut size = 0u32;
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_BINARY;
    if RegGetValueW(HKEY_CURRENT_USER, key, NAME, flags, None, None, Some(&mut size)).is_err() {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    RegGetValueW(HKEY_CURRENT_USER, key, NAME, flags, None, Some(data.as_mut_ptr().cast()), Some(&mut size)).ok().ok()?;
    data.truncate(size as usize);
    Some(data)
}

unsafe fn write(value: &str) {
    let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    let bytes = std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
    let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN, NAME, REG_SZ.0, Some(bytes.as_ptr().cast()), bytes.len() as u32);
}

unsafe fn delete(key: PCWSTR) {
    let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, key, NAME);
}

#[cfg(test)]
mod tests {
    use super::*;

    // F4: launch at startup.
    #[test]
    fn run_command_quotes_paths_with_spaces() {
        assert_eq!(command(r"C:\PROJECTS\Opennotch\release\Dynamic Island.exe"), r#""C:\PROJECTS\Opennotch\release\Dynamic Island.exe""#);
    }

    #[test]
    fn task_manager_disabled_counts_as_off() {
        assert!(approved(&[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(approved(&[0x06, 0, 0, 0]));
        assert!(!approved(&[0x03, 0, 0, 0, 0x12, 0x34]));
        assert!(!approved(&[0x07]));
        assert!(approved(&[])); // no data: Task Manager never touched it
    }

    #[test]
    fn registry_text_drops_trailing_nul() {
        let bytes: Vec<u8> = "\"C:\\a b.exe\"\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(text(&bytes), "\"C:\\a b.exe\"");
    }

    #[test]
    fn only_a_missing_exe_counts_as_stale() {
        let here = std::env::current_exe().unwrap();
        assert!(!stale(&command(&here.to_string_lossy())));
        assert!(stale(r#""C:\nowhere\moved away\Dynamic Island.exe""#));
    }
}
