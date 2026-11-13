//! "Start with Windows": the HKCU Run value, switched like Task Manager does through Explorer's
//! StartupApproved key. The MSI owns the Run value; we only switch it, so an upgrade that
//! rewrites it never turns a user's "off" back on. The registry is the state: no config key.

use crate::winutil::wide;
use clip_recorder::settings::{approved_entry, startup_approved};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_BINARY, REG_SZ, RRF_RT_ANY, RegGetValueW, RegSetKeyValueW,
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const NAME: &str = "Clip Recorder";

fn read(key: &str) -> Option<Vec<u8>> {
    let (key, name) = (wide(key), wide(NAME));
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_ANY,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        ) != 0
        {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        let ok = RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_ANY,
            std::ptr::null_mut(),
            data.as_mut_ptr().cast(),
            &mut size,
        ) == 0;
        ok.then(|| {
            data.truncate(size as usize);
            data
        })
    }
}

fn write(key: &str, kind: u32, data: &[u8]) -> bool {
    let (key, name) = (wide(key), wide(NAME));
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            kind,
            data.as_ptr().cast(),
            data.len() as u32,
        ) == 0
    }
}

pub fn enabled() -> bool {
    read(RUN).is_some() && startup_approved(read(APPROVED).as_deref())
}

/// On: the Run value points at this exe (re-pointed every time: an older install's path would
/// otherwise start nothing), approved. Off: marked disabled, the value left to the MSI.
pub fn set(on: bool) -> bool {
    if on {
        let Ok(exe) = std::env::current_exe() else { return false };
        let value: Vec<u8> = wide(&format!("\"{}\"", exe.display()))
            .iter()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        write(RUN, REG_SZ, &value) && write(APPROVED, REG_BINARY, &approved_entry(true, 0))
    } else if read(RUN).is_some() {
        let now = unsafe {
            let mut ft = std::mem::zeroed();
            windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime(&mut ft);
            (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
        };
        write(APPROVED, REG_BINARY, &approved_entry(false, now))
    } else {
        true
    }
}
