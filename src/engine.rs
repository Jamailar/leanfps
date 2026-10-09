//! Applies and reverts tweaks. Before every change the real previous value is read and
//! stored in a journal, so revert restores what the user had, not a hard-coded default.

use crate::catalog::{Action, Catalog, Form, Priority, RegData, Scope, Tweak};
use crate::platform::{file_name_lower, PResult, Platform};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// What was there before an action ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Prior {
    Registry { key: String, name: String, value: Option<RegData> },
    PowerScheme { scheme: String },
    PowerSetting { scheme: String, subgroup: String, setting: String, ac: u32, dc: u32 },
    Mouse { params: [i32; 3] },
    Priority { pid: u32, class: Priority },
    PriorityMany { items: Vec<(u32, Priority)> },
    Throttling { pid: u32 },
    KeepAwake,
    Overlay { overlay: String },
    Nothing,
}

/// The running game an action applies to.
#[derive(Debug, Clone, Copy)]
pub struct GameCtx<'a> {
    pub pid: u32,
    pub exe_path: &'a str,
}

pub const GPU_PREFS_KEY: &str = "HKCU\\Software\\Microsoft\\DirectX\\UserGpuPreferences";

/// Set `key=value;` inside a `a=1;b=2;` style string, keeping the other fields.
pub fn merge_kv(existing: &str, key: &str, value: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    for part in existing.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let k = part.split('=').next().unwrap_or("");
        if k.eq_ignore_ascii_case(key) {
            if !done {
                out.push_str(&format!("{key}={value};"));
                done = true;
            }
        } else {
            out.push_str(part);
            out.push(';');
        }
    }
    if !done {
        out.push_str(&format!("{key}={value};"));
    }
    out
}

/// IFEO launch-priority key for an exe: Windows applies `CpuPriorityClass` when it starts.
pub fn ifeo_key(exe_file: &str) -> String {
    format!("HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Image File Execution Options\\{exe_file}\\PerfOptions")
}

/// `CpuPriorityClass` values: High = 3, Above normal = 6, Normal = 2, Below normal = 5, Idle = 1.
pub fn ifeo_priority(p: Priority) -> u32 {
    match p {
        Priority::Idle => 1,
        Priority::Normal => 2,
        Priority::High => 3,
        Priority::BelowNormal => 5,
        Priority::AboveNormal => 6,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub tweak: String,
    pub prior: Prior,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    pub entries: Vec<Entry>,
}

impl Journal {
    pub fn has(&self, tweak: &str) -> bool {
        self.entries.iter().any(|e| e.tweak == tweak)
    }
}

const MOUSE_ACCEL_ON: [i32; 3] = [6, 10, 1];
const MOUSE_ACCEL_OFF: [i32; 3] = [0, 0, 0];

/// Run one action and return what it replaced. `game` is required for session actions.
pub fn apply_action(pf: &dyn Platform, action: &Action, game: Option<GameCtx>) -> PResult<Prior> {
    let need_game = || game.ok_or_else(|| "this change only works while a game is running".to_string());
    let need_pid = || need_game().map(|g| g.pid);
    match action {
        Action::Registry { key, name, value } => {
            let before = pf.reg_get(key, name)?;
            pf.reg_set(key, name, value)?;
            Ok(Prior::Registry { key: key.clone(), name: name.clone(), value: before })
        }
        Action::PowerScheme { scheme } => {
            if !pf.power_scheme_exists(scheme) {
                return Err("this power plan is not available on this PC".into());
            }
            let before = pf.power_active_scheme()?;
            pf.power_set_active_scheme(scheme)?;
            Ok(Prior::PowerScheme { scheme: before })
        }
        Action::PowerSetting { subgroup, setting, ac, dc } => {
            let scheme = pf.power_active_scheme()?;
            let (old_ac, old_dc) = pf.power_value(&scheme, subgroup, setting)?;
            pf.power_set_value(&scheme, subgroup, setting, *ac, dc.unwrap_or(old_dc))?;
            // Re-activating the scheme makes Windows pick up the new value.
            pf.power_set_active_scheme(&scheme)?;
            Ok(Prior::PowerSetting { scheme, subgroup: subgroup.clone(), setting: setting.clone(), ac: old_ac, dc: old_dc })
        }
        Action::MouseAcceleration { enabled } => {
            let before = pf.mouse_params()?;
            pf.set_mouse_params(if *enabled { MOUSE_ACCEL_ON } else { MOUSE_ACCEL_OFF })?;
            Ok(Prior::Mouse { params: before })
        }
        Action::GamePriority { class } => {
            let pid = need_pid()?;
            let before = pf.priority(pid)?;
            pf.set_priority(pid, *class)?;
            Ok(Prior::Priority { pid, class: before })
        }
        Action::GamePowerThrottlingOff => {
            let pid = need_pid()?;
            pf.set_power_throttling(pid, Some(false))?;
            Ok(Prior::Throttling { pid })
        }
        Action::KeepAwake => {
            pf.set_keep_awake(true)?;
            Ok(Prior::KeepAwake)
        }
        Action::BackgroundPriority { class, processes } => {
            let game = need_pid()?;
            let names: Vec<String> = processes.iter().map(|p| p.to_ascii_lowercase()).collect();
            let mut items = Vec::new();
            for p in pf.processes()? {
                if p.pid == game || !names.contains(&file_name_lower(&p.exe_name)) {
                    continue;
                }
                // Skip processes we can't read or change (other users, protected). Never fatal.
                if let Ok(before) = pf.priority(p.pid) {
                    if before > *class && pf.set_priority(p.pid, *class).is_ok() {
                        items.push((p.pid, before));
                    }
                }
            }
            Ok(Prior::PriorityMany { items })
        }
        Action::StandbyPurge { .. } => Ok(Prior::Nothing),
        Action::PowerOverlay { overlay } => {
            need_game()?;
            let before = pf.power_overlay()?;
            pf.set_power_overlay(overlay)?;
            Ok(Prior::Overlay { overlay: before })
        }
        Action::GpuPreference { value } => {
            let g = need_game()?;
            let before = pf.reg_get(GPU_PREFS_KEY, g.exe_path)?;
            let current = match &before {
                Some(RegData::Sz(s)) => s.clone(),
                _ => String::new(),
            };
            pf.reg_set(GPU_PREFS_KEY, g.exe_path, &RegData::Sz(merge_kv(&current, "GpuPreference", &value.to_string())))?;
            Ok(Prior::Registry { key: GPU_PREFS_KEY.into(), name: g.exe_path.into(), value: before })
        }
    }
}

pub fn revert_prior(pf: &dyn Platform, prior: &Prior) -> PResult<()> {
    match prior {
        Prior::Registry { key, name, value: Some(v) } => pf.reg_set(key, name, v),
        Prior::Registry { key, name, value: None } => pf.reg_delete(key, name),
        Prior::PowerScheme { scheme } => pf.power_set_active_scheme(scheme),
        Prior::PowerSetting { scheme, subgroup, setting, ac, dc } => {
            pf.power_set_value(scheme, subgroup, setting, *ac, *dc)?;
            if pf.power_active_scheme()? == *scheme {
                pf.power_set_active_scheme(scheme)?;
            }
            Ok(())
        }
        Prior::Mouse { params } => pf.set_mouse_params(*params),
        Prior::Priority { pid, class } => {
            if pf.is_alive(*pid) {
                pf.set_priority(*pid, *class)?;
            }
            Ok(())
        }
        Prior::PriorityMany { items } => {
            for (pid, class) in items {
                if pf.is_alive(*pid) {
                    let _ = pf.set_priority(*pid, *class);
                }
            }
            Ok(())
        }
        Prior::Throttling { pid } => {
            if pf.is_alive(*pid) {
                pf.set_power_throttling(*pid, None)?;
            }
            Ok(())
        }
        Prior::KeepAwake => pf.set_keep_awake(false),
        Prior::Overlay { overlay } => pf.set_power_overlay(overlay),
        Prior::Nothing => Ok(()),
    }
}

/// Is the persistent action's target value currently in effect?
pub fn action_in_effect(pf: &dyn Platform, action: &Action) -> PResult<bool> {
    Ok(match action {
        Action::Registry { key, name, value } => pf.reg_get(key, name)?.as_ref() == Some(value),
        Action::PowerScheme { scheme } => pf.power_active_scheme()?.eq_ignore_ascii_case(scheme),
        Action::PowerSetting { subgroup, setting, ac, dc } => {
            let scheme = pf.power_active_scheme()?;
            let (cur_ac, cur_dc) = pf.power_value(&scheme, subgroup, setting)?;
            cur_ac == *ac && dc.is_none_or(|d| d == cur_dc)
        }
        Action::MouseAcceleration { enabled } => (pf.mouse_params()?[2] != 0) == *enabled,
        _ => false,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TweakState {
    /// We applied it and it is still in effect.
    Applied,
    /// Windows is already set this way; nothing to do.
    AlreadySet,
    NotApplied,
    /// We applied it but something else changed it back.
    ChangedOutside,
    /// Session tweak: shown as on/off, applied while a game runs.
    Session,
    Unavailable(String),
}

pub struct Engine {
    pub catalog: Catalog,
    pub pf: Arc<dyn Platform>,
    pub journal: Journal,
    store: Store,
}

impl Engine {
    pub fn new(catalog: Catalog, pf: Arc<dyn Platform>, store: Store) -> Engine {
        let journal = store.load("journal.json").unwrap_or_default();
        Engine { catalog, pf, journal, store }
    }

    fn save(&self) -> PResult<()> {
        self.store.save("journal.json", &self.journal)
    }

    pub fn unavailable_reason(&self, t: &Tweak) -> Option<String> {
        let m = self.pf.machine();
        match t.form {
            Form::Desktop if m.laptop => return Some("Desktop PCs only".into()),
            Form::Laptop if !m.laptop => return Some("Laptops only".into()),
            _ => {}
        }
        if t.min_build > 0 && m.os_build > 0 && m.os_build < t.min_build {
            return Some("Needs a newer Windows version".into());
        }
        for a in &t.actions {
            if let Action::PowerScheme { scheme } = a {
                if !self.pf.power_scheme_exists(scheme) && !self.journal.has(&t.id) {
                    return Some("This power plan is not available on this PC".into());
                }
            }
        }
        None
    }

    pub fn state(&self, id: &str) -> TweakState {
        let Some(t) = self.catalog.get(id) else {
            return TweakState::Unavailable("unknown".into());
        };
        if t.scope == Scope::Session {
            return TweakState::Session;
        }
        let ours = self.journal.has(id);
        if !ours {
            if let Some(r) = self.unavailable_reason(t) {
                return TweakState::Unavailable(r);
            }
        }
        let in_effect = t.actions.iter().all(|a| action_in_effect(&*self.pf, a).unwrap_or(false));
        match (ours, in_effect) {
            (true, true) => TweakState::Applied,
            (true, false) => TweakState::ChangedOutside,
            (false, true) => TweakState::AlreadySet,
            (false, false) => TweakState::NotApplied,
        }
    }

    /// Apply a persistent tweak. All-or-nothing: on failure, the actions already done are undone.
    pub fn apply(&mut self, id: &str) -> PResult<()> {
        let t = self.catalog.get(id).ok_or("unknown tweak")?.clone();
        if t.scope != Scope::Persistent {
            return Err("applied automatically while a game runs".into());
        }
        if let Some(r) = self.unavailable_reason(&t) {
            return Err(r);
        }
        if self.journal.has(id) {
            // Re-apply after an outside change: restore first so the journal keeps the user's original value.
            self.revert(id)?;
        }
        let mut done: Vec<Prior> = Vec::new();
        for a in &t.actions {
            match apply_action(&*self.pf, a, None) {
                Ok(p) => done.push(p),
                Err(e) => {
                    for p in done.iter().rev() {
                        let _ = revert_prior(&*self.pf, p);
                    }
                    return Err(e);
                }
            }
        }
        self.journal.entries.extend(done.into_iter().map(|prior| Entry { tweak: id.into(), prior }));
        self.save()
    }

    pub fn revert(&mut self, id: &str) -> PResult<()> {
        let mut first_err = None;
        let entries: Vec<Entry> = self.journal.entries.iter().filter(|e| e.tweak == id).cloned().collect();
        let mut failed = Vec::new();
        for e in entries.iter().rev() {
            if let Err(err) = revert_prior(&*self.pf, &e.prior) {
                first_err.get_or_insert(err);
                failed.push(e.clone());
            }
        }
        self.journal.entries.retain(|e| e.tweak != id);
        // Keep what could not be restored so the user can retry.
        self.journal.entries.extend(failed);
        self.save()?;
        first_err.map_or(Ok(()), Err)
    }

    /// Revert every persistent tweak, newest first. Returns (tweak, error) for failures.
    pub fn revert_all(&mut self) -> Vec<(String, String)> {
        let mut ids: Vec<String> = Vec::new();
        for e in self.journal.entries.iter().rev() {
            if !ids.contains(&e.tweak) {
                ids.push(e.tweak.clone());
            }
        }
        ids.into_iter().filter_map(|id| self.revert(&id).err().map(|e| (id, e))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mock::{MockPlatform, BALANCED, HIGH_PERF};

    fn engine() -> (Engine, Arc<MockPlatform>) {
        let pf = Arc::new(MockPlatform::new());
        let e = Engine::new(Catalog::builtin(), pf.clone(), Store::temp());
        (e, pf)
    }

    #[test]
    fn registry_roundtrip_restores_real_previous_value() {
        let (mut e, pf) = engine();
        // User had a custom value; the other value did not exist.
        pf.with(|s| s.reg.insert(("HKCU\\System\\GameConfigStore".into(), "GameDVR_Enabled".into()), RegData::Dword(7)));
        assert_eq!(e.state("game_dvr_off"), TweakState::NotApplied);
        e.apply("game_dvr_off").unwrap();
        assert_eq!(e.state("game_dvr_off"), TweakState::Applied);
        e.revert("game_dvr_off").unwrap();
        pf.with(|s| {
            assert_eq!(s.reg.get(&("HKCU\\System\\GameConfigStore".into(), "GameDVR_Enabled".into())), Some(&RegData::Dword(7)));
            assert!(!s.reg.keys().any(|(_, n)| n == "AppCaptureEnabled"), "absent value must be deleted again");
        });
        assert!(e.journal.entries.is_empty());
    }

    #[test]
    fn failure_rolls_back_partial_apply() {
        let (mut e, pf) = engine();
        pf.with(|s| s.fail.push("reg_set:AllowAutoGameMode".into()));
        assert!(e.apply("game_mode_on").is_err());
        pf.with(|s| assert!(s.reg.is_empty()));
        assert!(e.journal.entries.is_empty());
    }

    #[test]
    fn power_plan_and_core_parking() {
        let (mut e, pf) = engine();
        e.apply("power_high_performance").unwrap();
        e.apply("core_parking_off").unwrap();
        pf.with(|s| assert_eq!(s.active, HIGH_PERF));
        assert_eq!(e.state("core_parking_off"), TweakState::Applied);
        let errs = e.revert_all();
        assert!(errs.is_empty(), "{errs:?}");
        pf.with(|s| {
            assert_eq!(s.active, BALANCED);
            let v = s.power.values().next().unwrap();
            assert_eq!(*v, (10, 10), "core parking restored on the plan it was changed on");
        });
    }

    #[test]
    fn desktop_only_tweak_unavailable_on_laptop() {
        let (mut e, pf) = engine();
        pf.with(|s| s.machine.laptop = true);
        assert!(matches!(e.state("power_high_performance"), TweakState::Unavailable(_)));
        assert!(e.apply("power_high_performance").is_err());
    }

    #[test]
    fn already_set_and_changed_outside() {
        let (mut e, pf) = engine();
        pf.with(|s| {
            s.reg.insert(("HKCU\\Software\\Microsoft\\GameBar".into(), "AutoGameModeEnabled".into()), RegData::Dword(1));
            s.reg.insert(("HKCU\\Software\\Microsoft\\GameBar".into(), "AllowAutoGameMode".into()), RegData::Dword(1));
        });
        assert_eq!(e.state("game_mode_on"), TweakState::AlreadySet);
        e.apply("game_dvr_off").unwrap();
        pf.with(|s| s.reg.insert(("HKCU\\System\\GameConfigStore".into(), "GameDVR_Enabled".into()), RegData::Dword(1)));
        assert_eq!(e.state("game_dvr_off"), TweakState::ChangedOutside);
        // Re-applying keeps the original (absent) value in the journal.
        e.apply("game_dvr_off").unwrap();
        e.revert("game_dvr_off").unwrap();
        pf.with(|s| assert!(s.reg.keys().all(|(_, n)| n != "GameDVR_Enabled" && n != "AppCaptureEnabled")));
    }

    #[test]
    fn merge_kv_keeps_other_fields() {
        assert_eq!(merge_kv("", "GpuPreference", "2"), "GpuPreference=2;");
        assert_eq!(merge_kv("AutoHDREnable=0;GpuPreference=1;", "GpuPreference", "2"), "AutoHDREnable=0;GpuPreference=2;");
        assert_eq!(merge_kv("SwapEffectUpgradeEnable=1", "GpuPreference", "2"), "SwapEffectUpgradeEnable=1;GpuPreference=2;");
    }

    #[test]
    fn journal_survives_restart() {
        let pf = Arc::new(MockPlatform::new());
        let store = Store::temp();
        let mut e = Engine::new(Catalog::builtin(), pf.clone(), store.clone());
        e.apply("game_dvr_off").unwrap();
        let e2 = Engine::new(Catalog::builtin(), pf.clone(), store);
        assert_eq!(e2.state("game_dvr_off"), TweakState::Applied);
    }
}
