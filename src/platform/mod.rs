//! Everything LeanFPS touches on the OS goes through [`Platform`], so the engine can be
//! tested on any OS with [`mock::MockPlatform`].

use crate::catalog::{Priority, RegData};

pub mod mock;
#[cfg(windows)]
pub mod win;

pub type PResult<T> = Result<T, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub exe_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    pub pid: u32,
    pub exe_path: String,
    /// The window covers its whole monitor (exclusive or borderless fullscreen).
    pub fullscreen: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    pub laptop: bool,
    pub os_build: u32,
    pub cpu: String,
}

/// RGBA icon, square.
#[derive(Debug, Clone)]
pub struct Icon {
    pub size: usize,
    pub rgba: Vec<u8>,
}

pub trait Platform: Send + Sync {
    fn reg_get(&self, key: &str, name: &str) -> PResult<Option<RegData>>;
    fn reg_set(&self, key: &str, name: &str, value: &RegData) -> PResult<()>;
    fn reg_delete(&self, key: &str, name: &str) -> PResult<()>;

    fn power_active_scheme(&self) -> PResult<String>;
    fn power_scheme_exists(&self, scheme: &str) -> bool;
    fn power_set_active_scheme(&self, scheme: &str) -> PResult<()>;
    /// Returns (ac, dc).
    fn power_value(&self, scheme: &str, subgroup: &str, setting: &str) -> PResult<(u32, u32)>;
    fn power_set_value(&self, scheme: &str, subgroup: &str, setting: &str, ac: u32, dc: u32) -> PResult<()>;

    /// "Enhance pointer precision" state as the three SPI_GETMOUSE values.
    fn mouse_params(&self) -> PResult<[i32; 3]>;
    fn set_mouse_params(&self, params: [i32; 3]) -> PResult<()>;

    fn processes(&self) -> PResult<Vec<ProcInfo>>;
    fn is_alive(&self, pid: u32) -> bool;
    fn priority(&self, pid: u32) -> PResult<Priority>;
    fn set_priority(&self, pid: u32, class: Priority) -> PResult<()>;
    /// `Some(false)` = opt out of throttling, `None` = let Windows decide (the default).
    fn set_power_throttling(&self, pid: u32, throttle: Option<bool>) -> PResult<()>;
    /// Must be called from the thread that owns the session (Windows ties this to a thread).
    fn set_keep_awake(&self, on: bool) -> PResult<()>;
    fn free_memory_mb(&self) -> PResult<u64>;
    fn purge_standby_list(&self) -> PResult<()>;

    fn foreground(&self) -> Option<Foreground>;
    fn machine(&self) -> Machine;
    fn exe_icon(&self, exe_path: &str, size: usize) -> Option<Icon>;
}

/// The platform for the OS we are running on.
pub fn native() -> Box<dyn Platform> {
    #[cfg(windows)]
    {
        Box::new(win::WinPlatform::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(mock::MockPlatform::demo())
    }
}

/// Lower-case file name of a path, handling both separators.
pub fn file_name_lower(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_ascii_lowercase()
}
