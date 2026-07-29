use crate::MonitorInfo;
use inputsync_core::{Error, Result};
use std::sync::Mutex;

use windows::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};

/// `dwFlags` value set on the primary monitor in `MONITORINFO`.
/// (Equivalent to `MONITORINFOF_PRIMARY` in the Windows SDK headers.)
const MONITORINFOF_PRIMARY: u32 = 0x0000_0001;

pub fn enumerate() -> Result<Vec<MonitorInfo>> {
    let collected: Mutex<Vec<MonitorInfo>> = Mutex::new(Vec::new());
    let collected_ptr: *const Mutex<Vec<MonitorInfo>> = &collected;

    unsafe {
        let ok = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(monitor_enum_proc),
            LPARAM(collected_ptr as isize),
        );
        if !ok.as_bool() {
            return Err(Error::Platform("EnumDisplayMonitors failed".into()));
        }
    }
    Ok(collected.into_inner().unwrap_or_default())
}

unsafe extern "system" fn monitor_enum_proc(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    user: LPARAM,
) -> BOOL {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    if !GetMonitorInfoW(monitor, &mut info.monitorInfo as *mut _ as *mut MONITORINFO).as_bool() {
        return TRUE;
    }
    let rect = info.monitorInfo.rcMonitor;
    let name = String::from_utf16_lossy(
        &info
            .szDevice
            .split(|c| *c == 0)
            .next()
            .unwrap_or(&info.szDevice),
    );
    let entry = MonitorInfo {
        name,
        x: rect.left,
        y: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
        primary: (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0,
    };
    let collected = &*(user.0 as *const Mutex<Vec<MonitorInfo>>);
    if let Ok(mut guard) = collected.lock() {
        guard.push(entry);
    }
    TRUE
}
