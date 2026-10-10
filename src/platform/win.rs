//! Real Windows implementation. Plain Win32 calls, no PowerShell, no helper processes.

use super::*;
use std::ffi::c_void;
use ::windows::core::{GUID, HSTRING, PCWSTR, PWSTR};
use ::windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_FILE_NOT_FOUND, HANDLE, HLOCAL, HWND, LUID, RECT, STILL_ACTIVE, WIN32_ERROR};
use ::windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetMonitorInfoW, MonitorFromWindow, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use ::windows::Win32::Security::{AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_PROF_SINGLE_PROCESS_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES};
use ::windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use ::windows::Win32::System::Power::{
    GetSystemPowerStatus, PowerGetActiveScheme, PowerReadACValueIndex, PowerReadDCValueIndex, PowerReadFriendlyName, PowerSetActiveScheme,
    PowerWriteACValueIndex, PowerWriteDCValueIndex, SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
    SYSTEM_POWER_STATUS,
};
use ::windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
};
use ::windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use ::windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetPriorityClass, OpenProcess, OpenProcessToken, ProcessPowerThrottling, QueryFullProcessImageNameW,
    SetPriorityClass, SetProcessInformation, ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
};
use ::windows::Win32::UI::Shell::SHDefExtractIconW;
use ::windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetDesktopWindow, GetForegroundWindow, GetIconInfo, GetShellWindow, GetWindowRect, GetWindowThreadProcessId,
    SystemParametersInfoW, HICON, ICONINFO, SPIF_SENDCHANGE, SPI_GETMOUSE, SPI_SETMOUSE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

#[link(name = "ntdll")]
extern "system" {
    fn NtSetSystemInformation(class: i32, info: *const c_void, len: u32) -> i32;
}

const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 80;
const MEMORY_PURGE_STANDBY_LIST: i32 = 4;

pub struct WinPlatform;

impl WinPlatform {
    pub fn new() -> Self {
        WinPlatform
    }
}

fn win(e: WIN32_ERROR, what: &str) -> PResult<()> {
    if e.is_ok() {
        Ok(())
    } else {
        Err(format!("{what} failed ({})", e.0))
    }
}

fn split_key(key: &str) -> PResult<(HKEY, &str)> {
    if let Some(rest) = key.strip_prefix("HKCU\\") {
        Ok((HKEY_CURRENT_USER, rest))
    } else if let Some(rest) = key.strip_prefix("HKLM\\") {
        Ok((HKEY_LOCAL_MACHINE, rest))
    } else {
        Err(format!("unsupported registry root in {key}"))
    }
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn parse_guid(s: &str) -> PResult<GUID> {
    let hex: String = s.chars().filter(|c| *c != '-' && *c != '{' && *c != '}').collect();
    u128::from_str_radix(&hex, 16).map(GUID::from_u128).map_err(|_| format!("bad GUID {s}"))
}

fn fmt_guid(g: &GUID) -> String {
    let h = format!("{:032x}", g.to_u128());
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn open_process(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> PResult<Handle> {
    unsafe { OpenProcess(access, false, pid) }.map(Handle).map_err(|e| format!("cannot open process {pid}: {}", e.message()))
}

fn class_to_flag(p: Priority) -> ::windows::Win32::System::Threading::PROCESS_CREATION_FLAGS {
    match p {
        Priority::Idle => IDLE_PRIORITY_CLASS,
        Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
        Priority::Normal => NORMAL_PRIORITY_CLASS,
        Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        Priority::High => HIGH_PRIORITY_CLASS,
    }
}

fn exe_path_of(pid: u32) -> Option<String> {
    let h = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

impl Platform for WinPlatform {
    fn reg_get(&self, key: &str, name: &str) -> PResult<Option<RegData>> {
        let (root, sub) = split_key(key)?;
        let mut hk = HKEY::default();
        let r = unsafe { RegOpenKeyExW(root, &HSTRING::from(sub), None, KEY_READ, &mut hk) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        win(r, "open registry key")?;
        let hk = Key(hk);
        let name_w = HSTRING::from(name);
        let mut ty = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let r = unsafe { RegQueryValueExW(hk.0, &name_w, None, Some(&mut ty), None, Some(&mut size)) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        win(r, "read registry value size")?;
        let mut buf = vec![0u8; size as usize];
        win(unsafe { RegQueryValueExW(hk.0, &name_w, None, Some(&mut ty), Some(buf.as_mut_ptr()), Some(&mut size)) }, "read registry value")?;
        buf.truncate(size as usize);
        Ok(match ty {
            REG_DWORD if buf.len() == 4 => Some(RegData::Dword(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))),
            REG_SZ => {
                let wide: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let text = String::from_utf16_lossy(&wide).trim_end_matches('\0').to_string();
                // Only use the text form when writing it back gives the same bytes.
                let same = text.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).eq(buf.iter().copied());
                Some(if same { RegData::Sz(text) } else { RegData::Raw { ty: ty.0, data: buf } })
            }
            // Any other type is kept as raw bytes so undo restores it exactly.
            _ => Some(RegData::Raw { ty: ty.0, data: buf }),
        })
    }

    fn reg_set(&self, key: &str, name: &str, value: &RegData) -> PResult<()> {
        let (root, sub) = split_key(key)?;
        let mut hk = HKEY::default();
        win(
            unsafe { RegCreateKeyExW(root, &HSTRING::from(sub), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut hk, None) },
            "create registry key",
        )?;
        let hk = Key(hk);
        let (ty, data): (REG_VALUE_TYPE, Vec<u8>) = match value {
            RegData::Dword(v) => (REG_DWORD, v.to_le_bytes().to_vec()),
            RegData::Sz(s) => (REG_SZ, s.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).collect()),
            RegData::Raw { ty, data } => (REG_VALUE_TYPE(*ty), data.clone()),
        };
        win(unsafe { RegSetValueExW(hk.0, &HSTRING::from(name), None, ty, Some(&data)) }, "write registry value")
    }

    fn reg_delete(&self, key: &str, name: &str) -> PResult<()> {
        let (root, sub) = split_key(key)?;
        let mut hk = HKEY::default();
        let r = unsafe { RegOpenKeyExW(root, &HSTRING::from(sub), None, KEY_SET_VALUE, &mut hk) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        win(r, "open registry key")?;
        let hk = Key(hk);
        let r = unsafe { RegDeleteValueW(hk.0, &HSTRING::from(name)) };
        if r == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        win(r, "delete registry value")
    }

    fn reg_subkeys(&self, key: &str) -> Vec<String> {
        use ::windows::Win32::System::Registry::RegEnumKeyExW;
        let Ok((root, sub)) = split_key(key) else { return Vec::new() };
        let mut hk = HKEY::default();
        if unsafe { RegOpenKeyExW(root, &HSTRING::from(sub), None, KEY_READ, &mut hk) }.is_err() {
            return Vec::new();
        }
        let hk = Key(hk);
        let mut out = Vec::new();
        let mut buf = [0u16; 256];
        for i in 0.. {
            let mut len = buf.len() as u32;
            let r = unsafe { RegEnumKeyExW(hk.0, i, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
            if r.is_err() {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        out
    }

    fn power_active_scheme(&self) -> PResult<String> {
        let mut p: *mut GUID = std::ptr::null_mut();
        win(unsafe { PowerGetActiveScheme(None, &mut p) }, "read active power plan")?;
        let g = unsafe { *p };
        unsafe {
            let _ = LocalFree(Some(HLOCAL(p as *mut c_void)));
        }
        Ok(fmt_guid(&g))
    }

    fn power_scheme_exists(&self, scheme: &str) -> bool {
        let Ok(g) = parse_guid(scheme) else { return false };
        let mut size = 0u32;
        unsafe { PowerReadFriendlyName(None, Some(&g), None, None, None, &mut size) }.is_ok()
    }

    fn power_set_active_scheme(&self, scheme: &str) -> PResult<()> {
        let g = parse_guid(scheme)?;
        win(unsafe { PowerSetActiveScheme(None, Some(&g)) }, "switch power plan")
    }

    fn power_value(&self, scheme: &str, subgroup: &str, setting: &str) -> PResult<(u32, u32)> {
        let (s, sg, st) = (parse_guid(scheme)?, parse_guid(subgroup)?, parse_guid(setting)?);
        let (mut ac, mut dc) = (0u32, 0u32);
        win(unsafe { PowerReadACValueIndex(None, Some(&s), Some(&sg), Some(&st), &mut ac) }, "read power setting")?;
        win(WIN32_ERROR(unsafe { PowerReadDCValueIndex(None, Some(&s), Some(&sg), Some(&st), &mut dc) }), "read power setting")?;
        Ok((ac, dc))
    }

    fn power_set_value(&self, scheme: &str, subgroup: &str, setting: &str, ac: u32, dc: u32) -> PResult<()> {
        let (s, sg, st) = (parse_guid(scheme)?, parse_guid(subgroup)?, parse_guid(setting)?);
        win(unsafe { PowerWriteACValueIndex(None, &s, Some(&sg), Some(&st), ac) }, "write power setting")?;
        win(WIN32_ERROR(unsafe { PowerWriteDCValueIndex(None, &s, Some(&sg), Some(&st), dc) }), "write power setting")
    }

    fn mouse_params(&self) -> PResult<[i32; 3]> {
        let mut p = [0i32; 3];
        unsafe { SystemParametersInfoW(SPI_GETMOUSE, 0, Some(p.as_mut_ptr() as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) }
            .map_err(|e| format!("read mouse settings: {}", e.message()))?;
        Ok(p)
    }

    fn set_mouse_params(&self, mut params: [i32; 3]) -> PResult<()> {
        // No SPIF_UPDATEINIFILE: the change lives in memory only, so a crash or reboot never
        // leaves acceleration off by surprise.
        unsafe { SystemParametersInfoW(SPI_SETMOUSE, 0, Some(params.as_mut_ptr() as *mut c_void), SPIF_SENDCHANGE) }
            .map_err(|e| format!("change mouse settings: {}", e.message()))
    }

    fn processes(&self) -> PResult<Vec<ProcInfo>> {
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(|e| e.message().to_string())?;
        let snap = Handle(snap);
        let mut out = Vec::new();
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = unsafe { Process32FirstW(snap.0, &mut e) }.is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            out.push(ProcInfo { pid: e.th32ProcessID, exe_name: String::from_utf16_lossy(&e.szExeFile[..len]) });
            ok = unsafe { Process32NextW(snap.0, &mut e) }.is_ok();
        }
        Ok(out)
    }

    fn is_alive(&self, pid: u32) -> bool {
        let Ok(h) = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return false };
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(h.0, &mut code) }.is_ok() && code == STILL_ACTIVE.0 as u32
    }

    fn priority(&self, pid: u32) -> PResult<Priority> {
        let h = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let c = unsafe { GetPriorityClass(h.0) };
        Ok(match c {
            x if x == IDLE_PRIORITY_CLASS.0 => Priority::Idle,
            x if x == BELOW_NORMAL_PRIORITY_CLASS.0 => Priority::BelowNormal,
            x if x == ABOVE_NORMAL_PRIORITY_CLASS.0 => Priority::AboveNormal,
            x if x == HIGH_PRIORITY_CLASS.0 => Priority::High,
            0 => return Err(format!("cannot read priority of process {pid}")),
            // Normal, or Realtime which we never set and treat as normal for restore purposes.
            _ => Priority::Normal,
        })
    }

    fn set_priority(&self, pid: u32, class: Priority) -> PResult<()> {
        let h = open_process(pid, PROCESS_SET_INFORMATION)?;
        unsafe { SetPriorityClass(h.0, class_to_flag(class)) }.map_err(|e| format!("cannot change priority: {}", e.message()))
    }

    fn set_power_throttling(&self, pid: u32, throttle: Option<bool>) -> PResult<()> {
        let h = open_process(pid, PROCESS_SET_INFORMATION)?;
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: if throttle.is_some() { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
            StateMask: if throttle == Some(true) { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
        };
        unsafe {
            SetProcessInformation(
                h.0,
                ProcessPowerThrottling,
                &state as *const _ as *const c_void,
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
        }
        .map_err(|e| format!("cannot change power throttling: {}", e.message()))
    }

    fn set_keep_awake(&self, on: bool) -> PResult<()> {
        let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
        if unsafe { SetThreadExecutionState(flags) }.0 == 0 {
            return Err("cannot change sleep settings".into());
        }
        Ok(())
    }

    fn free_memory_mb(&self) -> PResult<u64> {
        let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        unsafe { GlobalMemoryStatusEx(&mut m) }.map_err(|e| e.message().to_string())?;
        Ok(m.ullAvailPhys / (1024 * 1024))
    }

    fn purge_standby_list(&self) -> PResult<()> {
        enable_privilege(SE_PROF_SINGLE_PROCESS_NAME)?;
        let cmd = MEMORY_PURGE_STANDBY_LIST;
        let status = unsafe { NtSetSystemInformation(SYSTEM_MEMORY_LIST_INFORMATION, &cmd as *const i32 as *const c_void, 4) };
        if status < 0 {
            return Err(format!("purge standby list failed (0x{:08x})", status as u32));
        }
        Ok(())
    }

    fn foreground(&self) -> Option<Foreground> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() || hwnd == unsafe { GetShellWindow() } || hwnd == unsafe { GetDesktopWindow() } {
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 || pid == std::process::id() {
            return None;
        }
        let exe_path = exe_path_of(pid)?;
        let (fullscreen, coverage) = monitor_coverage(hwnd);
        Some(Foreground { pid, exe_path, fullscreen, coverage })
    }

    fn uses_graphics_api(&self, pid: u32) -> Option<bool> {
        use ::windows::Win32::System::Diagnostics::ToolHelp::{Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE};
        const APIS: [&str; 7] = ["d3d9.dll", "d3d10.dll", "d3d10_1.dll", "d3d11.dll", "d3d12.dll", "vulkan-1.dll", "opengl32.dll"];
        let snap = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) }.ok()?);
        let mut e = MODULEENTRY32W { dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
        let mut ok = unsafe { Module32FirstW(snap.0, &mut e) }.is_ok();
        while ok {
            let len = e.szModule.iter().position(|c| *c == 0).unwrap_or(e.szModule.len());
            let name = String::from_utf16_lossy(&e.szModule[..len]).to_ascii_lowercase();
            if APIS.contains(&name.as_str()) {
                return Some(true);
            }
            ok = unsafe { Module32NextW(snap.0, &mut e) }.is_ok();
        }
        Some(false)
    }

    fn power_overlay(&self) -> PResult<String> {
        type Get = unsafe extern "system" fn(*mut GUID) -> u32;
        let f: Get = unsafe { std::mem::transmute(powrprof_fn(b"PowerGetEffectiveOverlayScheme\0")?) };
        let mut g = GUID::zeroed();
        let r = unsafe { f(&mut g) };
        if r != 0 {
            return Err(format!("read power mode failed ({r})"));
        }
        Ok(fmt_guid(&g))
    }

    fn set_power_overlay(&self, overlay: &str) -> PResult<()> {
        type Set = unsafe extern "system" fn(GUID) -> u32;
        let g = parse_guid(overlay)?;
        let f: Set = unsafe { std::mem::transmute(powrprof_fn(b"PowerSetActiveOverlayScheme\0")?) };
        let r = unsafe { f(g) };
        if r != 0 {
            return Err(format!("change power mode failed ({r})"));
        }
        Ok(())
    }

    fn self_usage(&self) -> (u64, u64) {
        use ::windows::Win32::Foundation::FILETIME;
        use ::windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use ::windows::Win32::System::Threading::GetProcessTimes;
        let h = unsafe { GetCurrentProcess() };
        let (mut c, mut x, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) / 10_000;
        let cpu_ms = if unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) }.is_ok() { ft(k) + ft(u) } else { 0 };
        let mut m = PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
        let mem_mb = if unsafe { GetProcessMemoryInfo(h, &mut m, m.cb) }.is_ok() { (m.WorkingSetSize / (1024 * 1024)) as u64 } else { 0 };
        (cpu_ms, mem_mb)
    }

    fn machine(&self) -> Machine {
        let mut ps = SYSTEM_POWER_STATUS::default();
        // BatteryFlag 128 = no system battery, 255 = unknown: both treated as desktop.
        let laptop = unsafe { GetSystemPowerStatus(&mut ps) }.is_ok() && ps.BatteryFlag != 128 && ps.BatteryFlag != 255;
        let os_build = match self.reg_get("HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", "CurrentBuildNumber") {
            Ok(Some(RegData::Sz(s))) => s.parse().unwrap_or(0),
            _ => 0,
        };
        let cpu = match self.reg_get("HKLM\\HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0", "ProcessorNameString") {
            Ok(Some(RegData::Sz(s))) => s.trim().to_string(),
            _ => String::new(),
        };
        Machine { laptop, os_build, cpu }
    }

    fn exe_icon(&self, exe_path: &str, size: usize) -> Option<Icon> {
        let mut hicon = HICON::default();
        unsafe { SHDefExtractIconW(&HSTRING::from(exe_path), 0, 0, Some(&mut hicon), None, size as u32) }.ok().ok()?;
        if hicon.is_invalid() {
            return None;
        }
        let icon = icon_to_rgba(hicon, size);
        unsafe {
            let _ = DestroyIcon(hicon);
        }
        icon
    }
}

/// (covers the whole monitor, share of the monitor covered)
fn monitor_coverage(hwnd: HWND) -> (bool, f32) {
    let mut r = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
        return (false, 0.0);
    }
    let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() {
        return (false, 0.0);
    }
    let m = mi.rcMonitor;
    let full = r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom;
    let iw = (r.right.min(m.right) - r.left.max(m.left)).max(0) as f32;
    let ih = (r.bottom.min(m.bottom) - r.top.max(m.top)).max(0) as f32;
    let area = ((m.right - m.left) as f32 * (m.bottom - m.top) as f32).max(1.0);
    (full, (iw * ih / area).min(1.0))
}

/// Undocumented powrprof exports (used by e.g. G-Helper). Looked up at runtime so a
/// Windows build without them just reports the feature as unavailable.
fn powrprof_fn(name: &[u8]) -> PResult<unsafe extern "system" fn() -> isize> {
    use ::windows::core::PCSTR;
    use ::windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let lib = unsafe { LoadLibraryW(&HSTRING::from("powrprof.dll")) }.map_err(|e| e.message().to_string())?;
    unsafe { GetProcAddress(lib, PCSTR(name.as_ptr())) }.ok_or_else(|| "power mode is not supported on this Windows version".to_string())
}

fn enable_privilege(name: PCWSTR) -> PResult<()> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token) }.map_err(|e| e.message().to_string())?;
    let token = Handle(token);
    let mut luid = LUID::default();
    unsafe { LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid) }.map_err(|e| e.message().to_string())?;
    let mut tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, ..Default::default() };
    tp.Privileges[0].Luid = luid;
    tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
    unsafe { AdjustTokenPrivileges(token.0, false, Some(&tp), 0, None, None) }.map_err(|e| format!("missing administrator rights: {}", e.message()))
}

fn icon_to_rgba(hicon: HICON, size: usize) -> Option<Icon> {
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(hicon, &mut info) }.ok()?;
    let dc = unsafe { CreateCompatibleDC(None) };
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            biHeight: -(size as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; size * size * 4];
    let lines = unsafe { GetDIBits(dc, info.hbmColor, 0, size as u32, Some(bgra.as_mut_ptr() as *mut c_void), &mut bmi, DIB_RGB_COLORS) };
    unsafe {
        let _ = DeleteDC(dc);
        let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
    }
    if lines == 0 {
        return None;
    }
    let has_alpha = bgra.chunks_exact(4).any(|p| p[3] != 0);
    let mut rgba = Vec::with_capacity(bgra.len());
    for p in bgra.chunks_exact(4) {
        rgba.extend_from_slice(&[p[2], p[1], p[0], if has_alpha { p[3] } else { 255 }]);
    }
    Some(Icon { size, rgba })
}

/// Returns false if another LeanFPS is already running. The handle is intentionally leaked:
/// the mutex must live as long as the process.
pub fn single_instance() -> bool {
    use ::windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use ::windows::Win32::System::Threading::CreateMutexW;
    let h = unsafe { CreateMutexW(None, false, &HSTRING::from("Local\\LeanFPS.SingleInstance")) };
    h.is_ok() && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
}

/// Where Steam is installed, from its own registry entry.
pub fn steam_root() -> Option<std::path::PathBuf> {
    match WinPlatform.reg_get("HKCU\\Software\\Valve\\Steam", "SteamPath") {
        Ok(Some(RegData::Sz(p))) if !p.is_empty() => Some(p.replace('/', "\\").into()),
        _ => None,
    }
}

pub fn epic_manifests() -> Option<std::path::PathBuf> {
    let pd = std::env::var("ProgramData").ok()?;
    let p = std::path::PathBuf::from(pd).join("Epic").join("EpicGamesLauncher").join("Data").join("Manifests");
    p.is_dir().then_some(p)
}

pub fn open_url(url: &str) {
    use ::windows::Win32::UI::Shell::ShellExecuteW;
    use ::windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !(url.starts_with("https://")) {
        return;
    }
    unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Bring the already-running LeanFPS window to the front.
pub fn focus_existing_window() {
    use ::windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE};
    if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), &HSTRING::from("LeanFPS")) } {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// Start LeanFPS at sign-in, elevated and hidden in the tray, via Task Scheduler
/// (`schtasks /RL HIGHEST` avoids a UAC prompt at every boot).
pub fn set_autostart(on: bool) -> PResult<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Full path, so a schtasks.exe next to leanfps.exe (e.g. in Downloads) is never run.
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let mut cmd = std::process::Command::new(std::path::Path::new(&system_root).join("System32").join("schtasks.exe"));
    if on {
        let tr = format!("\"{}\" --tray", exe.display());
        cmd.args(["/Create", "/F", "/TN", "LeanFPS", "/SC", "ONLOGON", "/RL", "HIGHEST", "/TR", &tr]);
    } else {
        cmd.args(["/Delete", "/F", "/TN", "LeanFPS"]);
    }
    let out = cmd.creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    // Deleting a task that doesn't exist is fine.
    if out.status.success() || !on {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
