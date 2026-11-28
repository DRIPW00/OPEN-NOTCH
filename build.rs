//! Gives the exe its icon (assets/icon.ico) with no extra tools or crates: turns the .ico
//! into a Windows resource file (.res), which the MSVC linker reads directly.
//! The icon is resource 1; the tray icon loads it from there too.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let ico = std::fs::read("assets/icon.ico").expect("assets/icon.ico is missing");
    let res = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icon.res");
    std::fs::write(&res, res_from_ico(&ico)).unwrap();
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const ICON_ID: u16 = 1;

/// .ico -> .res: each image becomes an RT_ICON (ids 1, 2, …), plus one RT_GROUP_ICON
/// (id 1) listing them, as the resource compiler would write.
fn res_from_ico(ico: &[u8]) -> Vec<u8> {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]);
    assert!(ico.len() >= 6 && u16_at(2) == 1, "not an .ico file");
    let count = u16_at(4) as usize;

    let mut res = Vec::new();
    entry(&mut res, 0, 0, 0, &[]); // a .res file starts with an empty entry
    let mut group = vec![0, 0, 1, 0];
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for k in 0..count {
        let e = 6 + 16 * k; // ICONDIRENTRY
        let (size, offset) = (u32_at(e + 8) as usize, u32_at(e + 12) as usize);
        let id = k as u16 + 1;
        entry(&mut res, RT_ICON, id, 0x1010, &ico[offset..offset + size]);
        group.extend_from_slice(&ico[e..e + 4]); // width, height, colours, reserved
        group.extend_from_slice(&u16_at(e + 4).max(1).to_le_bytes()); // planes (some .ico writers put 0)
        group.extend_from_slice(&ico[e + 6..e + 12]); // bit count, size
        group.extend_from_slice(&id.to_le_bytes()); // GRPICONDIRENTRY ends with the id
    }
    entry(&mut res, RT_GROUP_ICON, ICON_ID, 0x1030, &group);
    res
}

/// One resource: a 32-byte header (numeric type and name), the data, padding to 4 bytes.
fn entry(res: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    let mut header = Vec::with_capacity(32);
    header.extend_from_slice(&(data.len() as u32).to_le_bytes()); // DataSize
    header.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    for n in [0xFFFF, kind, 0xFFFF, id] {
        header.extend_from_slice(&n.to_le_bytes()); // TYPE and NAME as numbers
    }
    header.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    header.extend_from_slice(&flags.to_le_bytes()); // MemoryFlags
    let language: u16 = if kind == 0 { 0 } else { 0x0409 };
    header.extend_from_slice(&language.to_le_bytes());
    header.extend_from_slice(&[0; 8]); // Version, Characteristics
    res.extend_from_slice(&header);
    res.extend_from_slice(data);
    while !res.len().is_multiple_of(4) {
        res.push(0);
    }
}
