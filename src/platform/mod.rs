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

#[derive(Debug, Clone, PartialEq)]
pub struct Foreground {
    pub pid: u32,
    pub exe_path: String,
    /// The window covers its whole monitor (exclusive or borderless fullscreen).
    pub fullscreen: bool,
    /// Share of the monitor the window covers, 0.0 to 1.0.
    pub coverage: f32,
}

/// One entry of the Windows "installed programs" list (Uninstall registry keys).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallEntry {
    pub key: String,
    pub display_name: String,
    pub publisher: String,
    pub install_location: String,
    pub uninstall_string: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    pub laptop: bool,
    pub os_build: u32,
    pub cpu: String,
    /// Two or more L3 caches of clearly different size (AMD X3D with one V-Cache die).
    pub split_cache: bool,
    /// Performance and efficiency cores (Intel 12th gen and later, some Arm and AMD chips).
    pub hybrid: bool,
}

/// One logical processor as a Windows CPU set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuSet {
    pub id: u32,
    /// Index of its last-level cache; logical processors sharing a cache share the index.
    pub llc: u8,
    /// Size of that L3 cache in KB (0 if unknown).
    pub l3_kb: u32,
    /// Higher = faster core. All equal on non-hybrid CPUs.
    pub efficiency: u8,
}

/// Which cores a game should run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuPrefer {
    /// The die with the biggest L3 cache (3D V-Cache).
    LargestCache,
    /// Performance cores only.
    PerformanceCores,
}

/// CPU set ids for `prefer`, or None when this CPU has no such split (then nothing to do).
pub fn cpu_target(sets: &[CpuSet], prefer: CpuPrefer) -> Option<Vec<u32>> {
    let (best, worst): (Box<dyn Fn(&CpuSet) -> bool>, bool) = match prefer {
        CpuPrefer::LargestCache => {
            let max = sets.iter().map(|s| s.l3_kb).max()?;
            let min = sets.iter().map(|s| s.l3_kb).min()?;
            // X3D: 96 MB vs 32 MB. Ignore small differences and unknown sizes.
            (Box::new(move |s: &CpuSet| s.l3_kb == max), min > 0 && max >= min + min / 2)
        }
        CpuPrefer::PerformanceCores => {
            let max = sets.iter().map(|s| s.efficiency).max()?;
            let min = sets.iter().map(|s| s.efficiency).min()?;
            (Box::new(move |s: &CpuSet| s.efficiency == max), max > min)
        }
    };
    if !worst {
        return None;
    }
    let ids: Vec<u32> = sets.iter().filter(|s| best(s)).map(|s| s.id).collect();
    // Never squeeze a game onto fewer than 4 logical processors.
    (ids.len() >= 4 && ids.len() < sets.len()).then_some(ids)
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
    /// Names of the direct subkeys of `key` (empty if it doesn't exist).
    fn reg_subkeys(&self, key: &str) -> Vec<String>;

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
    /// Whether the process has a 3D graphics API loaded (Direct3D, Vulkan, OpenGL).
    /// `None` when the process can't be inspected (e.g. anti-cheat protected).
    fn uses_graphics_api(&self, pid: u32) -> Option<bool>;
    /// Current Windows power mode overlay ("Best performance" etc.), if the API exists.
    fn power_overlay(&self) -> PResult<String>;
    fn set_power_overlay(&self, overlay: &str) -> PResult<()>;
    /// Logical processors as CPU sets (empty if Windows can't tell).
    fn cpu_sets(&self) -> Vec<CpuSet>;
    /// The process's default CPU sets; empty = all cores (the normal state).
    fn process_cpu_sets(&self, pid: u32) -> PResult<Vec<u32>>;
    /// Set the process's default CPU sets; empty restores all cores.
    fn set_process_cpu_sets(&self, pid: u32, ids: &[u32]) -> PResult<()>;
    /// This process's CPU time in milliseconds and working set in MB.
    fn self_usage(&self) -> (u64, u64);
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

/// The installed-programs list, read from the registry.
pub fn uninstall_entries(pf: &dyn Platform) -> Vec<UninstallEntry> {
    let roots = [
        "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
        "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    ];
    let text = |key: &str, name: &str| match pf.reg_get(key, name) {
        Ok(Some(RegData::Sz(s))) => s,
        _ => String::new(),
    };
    let mut out = Vec::new();
    for root in roots {
        for sub in pf.reg_subkeys(root) {
            let k = format!("{root}\\{sub}");
            out.push(UninstallEntry {
                display_name: text(&k, "DisplayName"),
                publisher: text(&k, "Publisher"),
                install_location: text(&k, "InstallLocation"),
                uninstall_string: text(&k, "UninstallString"),
                key: sub,
            });
        }
    }
    out
}

/// Lower-case file name of a path, handling both separators.
pub fn file_name_lower(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(n: u32, f: impl Fn(u32) -> (u32, u8)) -> Vec<CpuSet> {
        (0..n).map(|i| CpuSet { id: 256 + i, llc: (i / 16) as u8, l3_kb: f(i).0, efficiency: f(i).1 }).collect()
    }

    #[test]
    fn picks_vcache_die_and_performance_cores() {
        // 7950X3D: 32 logical processors, first die has 96 MB L3, second 32 MB.
        let x3d = cpu(32, |i| (if i < 16 { 98304 } else { 32768 }, 0));
        assert_eq!(cpu_target(&x3d, CpuPrefer::LargestCache), Some((256..272).collect()));
        assert_eq!(cpu_target(&x3d, CpuPrefer::PerformanceCores), None);
        // 7950X: same cache on both dies. 7800X3D: one die. Nothing to do.
        assert_eq!(cpu_target(&cpu(32, |_| (32768, 0)), CpuPrefer::LargestCache), None);
        assert_eq!(cpu_target(&cpu(16, |_| (98304, 0)), CpuPrefer::LargestCache), None);
        // Unknown cache sizes: never guess.
        assert_eq!(cpu_target(&cpu(32, |i| (if i < 16 { 98304 } else { 0 }, 0)), CpuPrefer::LargestCache), None);
        // i7-12700K: 16 P threads (class 1), 4 E cores (class 0).
        let hybrid = cpu(20, |i| (25600, if i < 16 { 1 } else { 0 }));
        assert_eq!(cpu_target(&hybrid, CpuPrefer::PerformanceCores).map(|v| v.len()), Some(16));
        // A chip with only 2 performance threads: don't squeeze the game onto them.
        assert_eq!(cpu_target(&cpu(10, |i| (8192, if i < 2 { 1 } else { 0 })), CpuPrefer::PerformanceCores), None);
        assert_eq!(cpu_target(&[], CpuPrefer::LargestCache), None);
    }
}
