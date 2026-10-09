//! In-memory platform used by tests and by non-Windows dev builds.

use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
pub const HIGH_PERF: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";

#[derive(Default)]
pub struct MockState {
    pub reg: HashMap<(String, String), RegData>,
    pub schemes: Vec<String>,
    pub active: String,
    pub power: HashMap<(String, String, String), (u32, u32)>,
    pub mouse: [i32; 3],
    pub procs: Vec<(ProcInfo, Priority)>,
    pub throttling: HashMap<u32, Option<bool>>,
    pub keep_awake: bool,
    pub free_mb: u64,
    pub purges: u32,
    pub foreground: Option<Foreground>,
    pub machine: Machine,
    pub graphics: HashMap<u32, bool>,
    pub overlay: String,
    /// Operations that should fail, e.g. "set_priority:42".
    pub fail: Vec<String>,
}

pub struct MockPlatform {
    pub state: Mutex<MockState>,
}

impl MockPlatform {
    pub fn new() -> Self {
        let s = MockState {
            schemes: vec![BALANCED.into(), HIGH_PERF.into()],
            active: BALANCED.into(),
            mouse: [6, 10, 1],
            free_mb: 8000,
            overlay: "00000000-0000-0000-0000-000000000000".into(),
            machine: Machine { laptop: false, os_build: 22631, cpu: "Mock CPU".into() },
            ..Default::default()
        };
        MockPlatform { state: Mutex::new(s) }
    }

    /// A mock with a few processes and a game in the foreground, for UI development off Windows.
    pub fn demo() -> Self {
        let m = Self::new();
        {
            let mut s = m.state.lock().unwrap();
            s.procs = vec![
                (ProcInfo { pid: 100, exe_name: "chrome.exe".into() }, Priority::Normal),
                (ProcInfo { pid: 200, exe_name: "cs2.exe".into() }, Priority::Normal),
            ];
            s.foreground = Some(Foreground {
                pid: 200,
                exe_path: r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe".into(),
                fullscreen: true,
                coverage: 1.0,
            });
        }
        m
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut MockState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    fn check(&self, op: &str) -> PResult<()> {
        if self.state.lock().unwrap().fail.iter().any(|f| f == op) {
            Err(format!("mock failure: {op}"))
        } else {
            Ok(())
        }
    }
}

impl Default for MockPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for MockPlatform {
    fn reg_get(&self, key: &str, name: &str) -> PResult<Option<RegData>> {
        Ok(self.with(|s| s.reg.get(&(key.into(), name.into())).cloned()))
    }
    fn reg_set(&self, key: &str, name: &str, value: &RegData) -> PResult<()> {
        self.check(&format!("reg_set:{name}"))?;
        self.with(|s| s.reg.insert((key.into(), name.into()), value.clone()));
        Ok(())
    }
    fn reg_delete(&self, key: &str, name: &str) -> PResult<()> {
        self.with(|s| s.reg.remove(&(key.into(), name.into())));
        Ok(())
    }
    fn reg_subkeys(&self, key: &str) -> Vec<String> {
        let prefix = format!("{key}\\");
        let mut v: Vec<String> = self.with(|s| {
            s.reg.keys().filter_map(|(k, _)| k.strip_prefix(&prefix).map(|r| r.split('\\').next().unwrap_or(r).to_string())).collect()
        });
        v.sort();
        v.dedup();
        v
    }
    fn power_active_scheme(&self) -> PResult<String> {
        Ok(self.with(|s| s.active.clone()))
    }
    fn power_scheme_exists(&self, scheme: &str) -> bool {
        self.with(|s| s.schemes.iter().any(|x| x == scheme))
    }
    fn power_set_active_scheme(&self, scheme: &str) -> PResult<()> {
        self.with(|s| {
            if !s.schemes.iter().any(|x| x == scheme) {
                return Err(format!("no scheme {scheme}"));
            }
            s.active = scheme.into();
            Ok(())
        })
    }
    fn power_value(&self, scheme: &str, sub: &str, setting: &str) -> PResult<(u32, u32)> {
        Ok(self.with(|s| *s.power.get(&(scheme.into(), sub.into(), setting.into())).unwrap_or(&(10, 10))))
    }
    fn power_set_value(&self, scheme: &str, sub: &str, setting: &str, ac: u32, dc: u32) -> PResult<()> {
        self.with(|s| s.power.insert((scheme.into(), sub.into(), setting.into()), (ac, dc)));
        Ok(())
    }
    fn mouse_params(&self) -> PResult<[i32; 3]> {
        Ok(self.with(|s| s.mouse))
    }
    fn set_mouse_params(&self, params: [i32; 3]) -> PResult<()> {
        self.with(|s| s.mouse = params);
        Ok(())
    }
    fn processes(&self) -> PResult<Vec<ProcInfo>> {
        Ok(self.with(|s| s.procs.iter().map(|p| p.0.clone()).collect()))
    }
    fn is_alive(&self, pid: u32) -> bool {
        self.with(|s| s.procs.iter().any(|p| p.0.pid == pid))
    }
    fn priority(&self, pid: u32) -> PResult<Priority> {
        self.with(|s| s.procs.iter().find(|p| p.0.pid == pid).map(|p| p.1).ok_or_else(|| format!("no pid {pid}")))
    }
    fn set_priority(&self, pid: u32, class: Priority) -> PResult<()> {
        self.check(&format!("set_priority:{pid}"))?;
        self.with(|s| match s.procs.iter_mut().find(|p| p.0.pid == pid) {
            Some(p) => {
                p.1 = class;
                Ok(())
            }
            None => Err(format!("no pid {pid}")),
        })
    }
    fn set_power_throttling(&self, pid: u32, throttle: Option<bool>) -> PResult<()> {
        self.with(|s| s.throttling.insert(pid, throttle));
        Ok(())
    }
    fn set_keep_awake(&self, on: bool) -> PResult<()> {
        self.with(|s| s.keep_awake = on);
        Ok(())
    }
    fn free_memory_mb(&self) -> PResult<u64> {
        Ok(self.with(|s| s.free_mb))
    }
    fn purge_standby_list(&self) -> PResult<()> {
        self.with(|s| s.purges += 1);
        Ok(())
    }
    fn foreground(&self) -> Option<Foreground> {
        self.with(|s| s.foreground.clone())
    }
    fn uses_graphics_api(&self, pid: u32) -> Option<bool> {
        self.with(|s| s.graphics.get(&pid).copied())
    }
    fn power_overlay(&self) -> PResult<String> {
        Ok(self.with(|s| s.overlay.clone()))
    }
    fn set_power_overlay(&self, overlay: &str) -> PResult<()> {
        self.check("set_power_overlay")?;
        self.with(|s| s.overlay = overlay.into());
        Ok(())
    }
    fn self_usage(&self) -> (u64, u64) {
        (0, 0)
    }
    fn machine(&self) -> Machine {
        self.with(|s| s.machine.clone())
    }
    fn exe_icon(&self, _exe_path: &str, size: usize) -> Option<Icon> {
        // A simple gradient square so the UI has something to draw.
        let mut rgba = Vec::with_capacity(size * size * 4);
        for y in 0..size {
            for x in 0..size {
                rgba.extend_from_slice(&[(40 + x * 150 / size) as u8, (90 + y * 120 / size) as u8, 200, 255]);
            }
        }
        Some(Icon { size, rgba })
    }
}
