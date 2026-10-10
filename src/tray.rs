//! Notification-area (tray) icon: grey while idle, green while a game is being optimized.
//! Left click opens the window; right click shows Open / Restore all / Quit.
//! Runs its own tiny message loop on a separate thread; no extra crates.

use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    Open,
    RestoreAll,
    Quit,
}

pub struct Tray {
    #[cfg(windows)]
    hwnd: std::sync::Arc<std::sync::atomic::AtomicIsize>,
}

#[cfg(not(windows))]
impl Tray {
    pub fn start(_tx: Sender<TrayEvent>, _wake: Box<dyn Fn() + Send>) -> Tray {
        Tray {}
    }
    pub fn set_active(&self, _active: bool, _tip: &str) {}
    pub fn remove(&self) {}
}

/// Ask an already-running LeanFPS (possibly hidden in the tray) to show its window.
#[cfg(windows)]
pub fn wake_existing() -> bool {
    use ::windows::core::HSTRING;
    use ::windows::Win32::Foundation::{LPARAM, WPARAM};
    use ::windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, PostMessageW, HWND_MESSAGE};
    match unsafe { FindWindowExW(Some(HWND_MESSAGE), None, &HSTRING::from(win::CLASS), None) } {
        Ok(h) => unsafe { PostMessageW(Some(h), win::MSG_OPEN, WPARAM(0), LPARAM(0)) }.is_ok(),
        Err(_) => false,
    }
}

#[cfg(windows)]
impl Tray {
    pub fn start(tx: Sender<TrayEvent>, wake: Box<dyn Fn() + Send>) -> Tray {
        let hwnd = std::sync::Arc::new(std::sync::atomic::AtomicIsize::new(0));
        let slot = hwnd.clone();
        std::thread::Builder::new().name("tray".into()).spawn(move || win::run(tx, wake, slot)).expect("start tray");
        Tray { hwnd }
    }

    /// Take the icon out of the notification area (on quit).
    pub fn remove(&self) {
        use ::windows::Win32::Foundation::HWND;
        use ::windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};
        let h = self.hwnd.load(std::sync::atomic::Ordering::SeqCst);
        if h != 0 {
            let d = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: HWND(h as *mut _), uID: 1, ..Default::default() };
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &d);
            }
        }
    }

    /// Switch icon and tooltip. Safe to call from any thread.
    pub fn set_active(&self, active: bool, tip: &str) {
        use ::windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use ::windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        *win::TIP.lock().unwrap() = tip.to_string();
        let h = self.hwnd.load(std::sync::atomic::Ordering::SeqCst);
        if h != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(h as *mut _)), win::MSG_SET, WPARAM(active as usize), LPARAM(0));
            }
        }
    }
}

#[cfg(windows)]
mod win {
    use super::TrayEvent;
    use ::windows::core::{HSTRING, PCWSTR};
    use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use ::windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};
    use ::windows::Win32::UI::WindowsAndMessaging::*;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex, OnceLock};

    pub const CLASS: &str = "LeanFPS.Tray";
    const MSG_TRAY: u32 = WM_APP + 1;
    pub const MSG_SET: u32 = WM_APP + 2;
    pub const MSG_OPEN: u32 = WM_APP + 3;
    const ID_OPEN: usize = 1;
    const ID_RESTORE: usize = 2;
    const ID_QUIT: usize = 3;
    const ICON_IDLE: u16 = 101;
    const ICON_ACTIVE: u16 = 102;

    pub static TIP: Mutex<String> = Mutex::new(String::new());
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
    static SINK: OnceLock<(Mutex<Sender<TrayEvent>>, Box<dyn Fn() + Send + Sync>)> = OnceLock::new();

    fn send(e: TrayEvent) {
        if let Some((tx, wake)) = SINK.get() {
            let _ = tx.lock().unwrap().send(e);
            wake();
        }
    }

    fn icon(id: u16) -> HICON {
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
        let inst = unsafe { GetModuleHandleW(PCWSTR::null()) }.ok();
        match unsafe { LoadImageW(inst.map(|m| m.into()), PCWSTR(id as usize as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) } {
            Ok(h) => HICON(h.0),
            Err(_) => HICON::default(),
        }
    }

    fn data(hwnd: HWND) -> NOTIFYICONDATAW {
        let mut d = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: MSG_TRAY,
            hIcon: icon(if ACTIVE.load(Ordering::SeqCst) { ICON_ACTIVE } else { ICON_IDLE }),
            ..Default::default()
        };
        let tip = TIP.lock().unwrap();
        let text = if tip.is_empty() { "LeanFPS" } else { tip.as_str() };
        for (i, c) in text.encode_utf16().take(d.szTip.len() - 1).enumerate() {
            d.szTip[i] = c;
        }
        d
    }

    fn menu(hwnd: HWND) {
        unsafe {
            let Ok(m) = CreatePopupMenu() else { return };
            let _ = AppendMenuW(m, MF_STRING, ID_OPEN, &HSTRING::from("Open LeanFPS"));
            let _ = AppendMenuW(m, MF_STRING, ID_RESTORE, &HSTRING::from("Restore all changes"));
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(m, MF_STRING, ID_QUIT, &HSTRING::from("Quit"));
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Needed so the menu closes when clicking elsewhere.
            let _ = SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(m, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, None, hwnd, None).0 as usize;
            let _ = DestroyMenu(m);
            match cmd {
                ID_OPEN => send(TrayEvent::Open),
                ID_RESTORE => send(TrayEvent::RestoreAll),
                ID_QUIT => send(TrayEvent::Quit),
                _ => {}
            }
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            MSG_TRAY => {
                match (lp.0 as u32) & 0xffff {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => send(TrayEvent::Open),
                    WM_RBUTTONUP => menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            MSG_SET => {
                ACTIVE.store(wp.0 != 0, Ordering::SeqCst);
                let _ = Shell_NotifyIconW(NIM_MODIFY, &data(hwnd));
                LRESULT(0)
            }
            MSG_OPEN => {
                send(TrayEvent::Open);
                LRESULT(0)
            }
            m if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => {
                // Explorer restarted: the icon is gone, add it again.
                let _ = Shell_NotifyIconW(NIM_ADD, &data(hwnd));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    pub fn run(tx: Sender<TrayEvent>, wake: Box<dyn Fn() + Send>, slot: Arc<AtomicIsize>) {
        // `wake` only calls egui's request_repaint, which is thread-safe.
        struct SyncWake(Box<dyn Fn() + Send>);
        unsafe impl Sync for SyncWake {}
        impl SyncWake {
            fn call(&self) {
                (self.0)()
            }
        }
        let w = SyncWake(wake);
        let _ = SINK.set((Mutex::new(tx), Box::new(move || w.call())));
        unsafe {
            TASKBAR_CREATED.store(RegisterWindowMessageW(&HSTRING::from("TaskbarCreated")), Ordering::SeqCst);
            let inst = GetModuleHandleW(PCWSTR::null()).ok();
            let class = HSTRING::from(CLASS);
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst.map(|m| m.into()).unwrap_or_default(), lpszClassName: PCWSTR(class.as_ptr()), ..Default::default() };
            RegisterClassW(&wc);
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                &class,
                &HSTRING::from("LeanFPS tray"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                inst.map(|m| m.into()),
                None,
            ) else {
                return;
            };
            slot.store(hwnd.0 as isize, Ordering::SeqCst);
            let _ = Shell_NotifyIconW(NIM_ADD, &data(hwnd));
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = Shell_NotifyIconW(NIM_DELETE, &data(hwnd));
        }
    }
}
