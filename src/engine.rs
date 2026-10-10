//! Applies and reverts tweaks. Before every change the real previous value is read and
//! stored in a journal, so revert restores what the user had, not a hard-coded default.

use crate::catalog::{Action, Catalog, CpuNeed, Form, Priority, RegData, Scope, Tweak, PROTECTED_PROCESSES};
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
    /// The game's CPU sets before (empty = all cores).
    CpuSets { pid: u32, ids: Vec<u32> },
    /// One `field=value;` entry of a registry string: its old value (None = not present) and
    /// whether the registry value existed at all.
    RegistryField { key: String, name: String, field: String, old: Option<String>, existed: bool },
    Nothing,
}

/// The running game an action applies to.
#[derive(Debug, Clone, Copy)]
pub struct GameCtx<'a> {
    pub pid: u32,
    pub exe_path: &'a str,
}

const JOURNAL_FILE: &str = "journal.json";

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

/// Value of `key` inside a `a=1;b=2;` style string.
pub fn get_kv(existing: &str, key: &str) -> Option<String> {
    existing.split(';').map(str::trim).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim().to_string())
    })
}

/// Remove `key=...;` from a `a=1;b=2;` style string, keeping the other fields.
pub fn remove_kv(existing: &str, key: &str) -> String {
    let mut out = String::new();
    for part in existing.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        if !part.split('=').next().unwrap_or("").trim().eq_ignore_ascii_case(key) {
            out.push_str(part);
            out.push(';');
        }
    }
    out
}

fn reg_text(v: &Option<RegData>) -> PResult<String> {
    match v {
        None => Ok(String::new()),
        Some(RegData::Sz(s)) => Ok(s.clone()),
        Some(_) => Err("unexpected registry value type; left unchanged".into()),
    }
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
///
/// Record first, then modify: reads what is there now, hands it to `record` (which must
/// persist it), and only changes the system once `record` succeeded. If `record` fails nothing
/// is touched. If the change itself fails, the prior stays recorded and the caller undoes it
/// with [`undo_failed`], so a half-done change is never left without its undo data.
pub fn apply_action_recorded(
    pf: &dyn Platform,
    action: &Action,
    game: Option<GameCtx>,
    record: &mut dyn FnMut(&Prior) -> PResult<()>,
) -> PResult<Prior> {
    let prior = capture_prior(pf, action, game)?;
    record(&prior).map_err(|e| format!("could not save undo information, nothing was changed: {e}"))?;
    perform(pf, action, game, &prior)?;
    Ok(prior)
}

/// Undo a recorded prior whose action failed part-way. Returns true when nothing is left to
/// restore, so the record can be dropped. Process-level changes die with the process, so a
/// failure there never leaves anything behind.
pub fn undo_failed(pf: &dyn Platform, prior: &Prior) -> bool {
    revert_prior(pf, prior).is_ok() || prior.is_process_scoped()
}

impl Prior {
    /// Lives only as long as one process (priority, throttling, keep-awake).
    pub fn is_process_scoped(&self) -> bool {
        matches!(
            self,
            Prior::Priority { .. } | Prior::PriorityMany { .. } | Prior::Throttling { .. } | Prior::KeepAwake | Prior::CpuSets { .. } | Prior::Nothing
        )
    }
}

/// Read the current state an action would replace. Changes nothing.
pub fn capture_prior(pf: &dyn Platform, action: &Action, game: Option<GameCtx>) -> PResult<Prior> {
    let need_game = || game.ok_or_else(|| "this change only works while a game is running".to_string());
    let need_pid = || need_game().map(|g| g.pid);
    match action {
        Action::Registry { key, name, .. } => Ok(Prior::Registry { key: key.clone(), name: name.clone(), value: pf.reg_get(key, name)? }),
        Action::PowerScheme { scheme } => {
            if !pf.power_scheme_exists(scheme) {
                return Err("this power plan is not available on this PC".into());
            }
            Ok(Prior::PowerScheme { scheme: pf.power_active_scheme()? })
        }
        Action::PowerSetting { subgroup, setting, .. } => {
            let scheme = pf.power_active_scheme()?;
            let (ac, dc) = pf.power_value(&scheme, subgroup, setting)?;
            Ok(Prior::PowerSetting { scheme, subgroup: subgroup.clone(), setting: setting.clone(), ac, dc })
        }
        Action::MouseAcceleration { .. } => Ok(Prior::Mouse { params: pf.mouse_params()? }),
        Action::GamePriority { .. } => {
            let pid = need_pid()?;
            Ok(Prior::Priority { pid, class: pf.priority(pid)? })
        }
        Action::GamePowerThrottlingOff => Ok(Prior::Throttling { pid: need_pid()? }),
        Action::KeepAwake => Ok(Prior::KeepAwake),
        Action::BackgroundPriority { class, processes } => {
            let game = need_pid()?;
            let names: Vec<String> = processes.iter().map(|p| p.to_ascii_lowercase()).collect();
            let mut items = Vec::new();
            for p in pf.processes()? {
                let exe = file_name_lower(&p.exe_name);
                if p.pid == game || !names.contains(&exe) || PROTECTED_PROCESSES.contains(&exe.as_str()) {
                    continue;
                }
                // Skip processes we can't read (other users, protected). Never fatal.
                if let Ok(before) = pf.priority(p.pid) {
                    if before > *class {
                        items.push((p.pid, before));
                    }
                }
            }
            Ok(Prior::PriorityMany { items })
        }
        Action::StandbyPurge { .. } => Ok(Prior::Nothing),
        Action::PowerOverlay { .. } => {
            need_game()?;
            Ok(Prior::Overlay { overlay: pf.power_overlay()? })
        }
        Action::GpuPreference { .. } => {
            let g = need_game()?;
            Ok(Prior::Registry { key: GPU_PREFS_KEY.into(), name: g.exe_path.into(), value: pf.reg_get(GPU_PREFS_KEY, g.exe_path)? })
        }
        Action::GameCpuSets { .. } => {
            let pid = need_pid()?;
            Ok(Prior::CpuSets { pid, ids: pf.process_cpu_sets(pid)? })
        }
        Action::RegistryField { key, name, field, .. } => {
            let v = pf.reg_get(key, name)?;
            let text = reg_text(&v)?;
            Ok(Prior::RegistryField { key: key.clone(), name: name.clone(), field: field.clone(), old: get_kv(&text, field), existed: v.is_some() })
        }
    }
}

/// Make the change. `prior` is what [`capture_prior`] returned for the same action.
fn perform(pf: &dyn Platform, action: &Action, game: Option<GameCtx>, prior: &Prior) -> PResult<()> {
    match (action, prior) {
        (Action::Registry { key, name, value }, _) => pf.reg_set(key, name, value),
        (Action::PowerScheme { scheme }, _) => pf.power_set_active_scheme(scheme),
        (Action::PowerSetting { subgroup, setting, ac, dc }, Prior::PowerSetting { scheme, dc: old_dc, .. }) => {
            pf.power_set_value(scheme, subgroup, setting, *ac, dc.unwrap_or(*old_dc))?;
            // Re-activating the scheme makes Windows pick up the new value.
            pf.power_set_active_scheme(scheme)
        }
        (Action::MouseAcceleration { enabled }, _) => pf.set_mouse_params(if *enabled { MOUSE_ACCEL_ON } else { MOUSE_ACCEL_OFF }),
        (Action::GamePriority { class }, Prior::Priority { pid, .. }) => pf.set_priority(*pid, *class),
        (Action::GamePowerThrottlingOff, Prior::Throttling { pid }) => pf.set_power_throttling(*pid, Some(false)),
        (Action::KeepAwake, _) => pf.set_keep_awake(true),
        (Action::BackgroundPriority { class, .. }, Prior::PriorityMany { items }) => {
            // Best effort: a process that refuses keeps its priority, and restoring it later
            // just sets the value it already has.
            for (pid, _) in items {
                let _ = pf.set_priority(*pid, *class);
            }
            Ok(())
        }
        (Action::StandbyPurge { .. }, _) => Ok(()),
        (Action::PowerOverlay { overlay }, _) => pf.set_power_overlay(overlay),
        (Action::GpuPreference { value }, Prior::Registry { value: before, .. }) => {
            let g = game.ok_or("this change only works while a game is running")?;
            let current = match before {
                Some(RegData::Sz(s)) => s.clone(),
                _ => String::new(),
            };
            pf.reg_set(GPU_PREFS_KEY, g.exe_path, &RegData::Sz(merge_kv(&current, "GpuPreference", &value.to_string())))
        }
        (Action::GameCpuSets { prefer }, Prior::CpuSets { pid, ids }) => {
            if !ids.is_empty() {
                // The user (or another tool) already picked cores for this game: leave it.
                return Ok(());
            }
            let target = crate::platform::cpu_target(&pf.cpu_sets(), *prefer).ok_or("not needed on this CPU")?;
            pf.set_process_cpu_sets(*pid, &target)
        }
        (Action::RegistryField { key, name, field, value }, Prior::RegistryField { .. }) => {
            // Re-read right before writing so a change made since the capture is kept.
            let current = reg_text(&pf.reg_get(key, name)?)?;
            pf.reg_set(key, name, &RegData::Sz(merge_kv(&current, field, value)))
        }
        _ => Err("internal error: undo information does not match the change".into()),
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
        Prior::CpuSets { pid, ids } => {
            if pf.is_alive(*pid) {
                pf.set_process_cpu_sets(*pid, ids)?;
            }
            Ok(())
        }
        Prior::RegistryField { key, name, field, old, existed } => {
            // Only our entry goes back; anything else changed in the meantime stays.
            let current = reg_text(&pf.reg_get(key, name)?)?;
            let text = match old {
                Some(v) => merge_kv(&current, field, v),
                None => remove_kv(&current, field),
            };
            if text.is_empty() && !existed {
                pf.reg_delete(key, name)
            } else {
                pf.reg_set(key, name, &RegData::Sz(text))
            }
        }
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
        Action::RegistryField { key, name, field, value } => get_kv(&reg_text(&pf.reg_get(key, name)?)?, field).as_deref() == Some(value.as_str()),
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
        let journal = store.load(JOURNAL_FILE).unwrap_or_default();
        Engine { catalog, pf, journal, store }
    }

    fn save(&self) -> PResult<()> {
        self.store.save(JOURNAL_FILE, &self.journal)
    }

    pub fn unavailable_reason(&self, t: &Tweak) -> Option<String> {
        let m = self.pf.machine();
        if !t.fits(&m) {
            return Some(match (t.form, t.cpu) {
                (Form::Desktop, _) if m.laptop => "Desktop PCs only".into(),
                (Form::Laptop, _) if !m.laptop => "Laptops only".into(),
                (_, CpuNeed::SplitCache) if !m.split_cache => "Only for CPUs with 3D V-Cache on one of two dies".into(),
                (_, CpuNeed::Hybrid) if !m.hybrid => "Only for CPUs with performance and efficiency cores".into(),
                _ => "Needs a newer Windows version".into(),
            });
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
            if let Some(r) = self.unavailable_reason(t) {
                return TweakState::Unavailable(r);
            }
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
        // Record first, then modify: each prior is in journal.json on disk before its change runs.
        let pf = self.pf.clone();
        let mut failure = None;
        for a in &t.actions {
            let (journal, store) = (&mut self.journal, &self.store);
            let mut record = |p: &Prior| {
                journal.entries.push(Entry { tweak: id.into(), prior: p.clone() });
                let r = store.save(JOURNAL_FILE, &*journal);
                if r.is_err() {
                    journal.entries.pop();
                }
                r
            };
            if let Err(e) = apply_action_recorded(&*pf, a, None, &mut record) {
                failure = Some(e);
                break;
            }
        }
        let Some(err) = failure else { return Ok(()) };
        // All-or-nothing: undo what this call did, newest first. Anything that can't be undone
        // stays in the journal so Restore all can retry it.
        let mut keep = Vec::new();
        let mut i = self.journal.entries.len();
        while i > 0 && self.journal.entries[i - 1].tweak == id {
            i -= 1;
        }
        for e in self.journal.entries.split_off(i).into_iter().rev() {
            if !undo_failed(&*pf, &e.prior) {
                keep.push(e);
            }
        }
        keep.reverse();
        self.journal.entries.extend(keep);
        let _ = self.save();
        Err(err)
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

    /// A store whose directory can't be created: every save fails.
    fn broken_store() -> Store {
        let blocker = std::env::temp_dir().join(format!("leanfps-blocker-{}-{}", std::process::id(), line!()));
        std::fs::write(&blocker, b"not a directory").unwrap();
        Store::new(blocker.join("data"))
    }

    #[test]
    fn nothing_changes_when_undo_info_cannot_be_saved() {
        let pf = Arc::new(MockPlatform::new());
        let mut e = Engine::new(Catalog::builtin(), pf.clone(), broken_store());
        assert!(e.apply("game_dvr_off").is_err());
        assert!(e.apply("power_high_performance").is_err());
        pf.with(|s| {
            assert!(s.reg.is_empty(), "registry touched without a saved journal");
            assert_eq!(s.active, BALANCED);
        });
        assert!(e.journal.entries.is_empty());
    }

    #[test]
    fn prior_is_on_disk_before_the_change() {
        // The first action of game_mode_on succeeds, the second fails: the journal on disk
        // must never miss a value that was changed.
        let (mut e, pf) = engine();
        pf.with(|s| s.fail.push("reg_set:AllowAutoGameMode".into()));
        assert!(e.apply("game_mode_on").is_err());
        pf.with(|s| assert!(s.reg.is_empty(), "partial change rolled back"));
        let on_disk: Journal = e.store.load(JOURNAL_FILE).unwrap_or_default();
        assert!(on_disk.entries.is_empty(), "rolled-back entries removed from disk");
    }

    #[test]
    fn other_registry_types_restore_byte_for_byte() {
        let (mut e, pf) = engine();
        let raw = RegData::Raw { ty: 11, data: vec![1, 2, 3, 4, 5, 6, 7, 8] };
        pf.with(|s| s.reg.insert(("HKCU\\System\\GameConfigStore".into(), "GameDVR_Enabled".into()), raw.clone()));
        e.apply("game_dvr_off").unwrap();
        // Through a restart, so the journal's JSON form is used.
        let mut e2 = Engine::new(Catalog::builtin(), pf.clone(), e.store.clone());
        e2.revert("game_dvr_off").unwrap();
        pf.with(|s| assert_eq!(s.reg.get(&("HKCU\\System\\GameConfigStore".into(), "GameDVR_Enabled".into())), Some(&raw)));
    }

    #[test]
    fn corrupt_journal_is_kept_not_overwritten() {
        let pf = Arc::new(MockPlatform::new());
        let store = Store::temp();
        std::fs::create_dir_all(store.path("")).unwrap();
        std::fs::write(store.path(JOURNAL_FILE), b"{ half a jour").unwrap();
        let mut e = Engine::new(Catalog::builtin(), pf.clone(), store.clone());
        e.apply("game_dvr_off").unwrap();
        let kept = std::fs::read_dir(store.path("")).unwrap().filter_map(|d| d.ok()).any(|d| {
            d.file_name().to_string_lossy().starts_with("journal.json.corrupt-") && std::fs::read(d.path()).unwrap() == b"{ half a jour"
        });
        assert!(kept, "the unreadable journal must be moved aside, not lost");
    }

    #[test]
    fn registry_field_keeps_other_entries_and_undoes_only_its_own() {
        let (mut e, pf) = engine();
        let k = || ("HKCU\\Software\\Microsoft\\DirectX\\UserGpuPreferences".to_string(), "DirectXUserGlobalSettings".to_string());
        // Nothing there before: undo deletes the value again.
        e.apply("windowed_flip_model").unwrap();
        assert_eq!(e.state("windowed_flip_model"), TweakState::Applied);
        pf.with(|s| assert_eq!(s.reg.get(&k()), Some(&RegData::Sz("SwapEffectUpgradeEnable=1;".into()))));
        e.revert("windowed_flip_model").unwrap();
        pf.with(|s| assert!(!s.reg.contains_key(&k())));

        // User had Auto HDR and the optimization off; turns VRR on while ours is applied.
        pf.with(|s| s.reg.insert(k(), RegData::Sz("AutoHDREnable=1;SwapEffectUpgradeEnable=0;".into())));
        e.apply("windowed_flip_model").unwrap();
        pf.with(|s| assert_eq!(s.reg.get(&k()), Some(&RegData::Sz("AutoHDREnable=1;SwapEffectUpgradeEnable=1;".into()))));
        pf.with(|s| s.reg.insert(k(), RegData::Sz("AutoHDREnable=1;SwapEffectUpgradeEnable=1;VRROptimizeEnable=1;".into())));
        e.revert("windowed_flip_model").unwrap();
        pf.with(|s| assert_eq!(s.reg.get(&k()), Some(&RegData::Sz("AutoHDREnable=1;SwapEffectUpgradeEnable=0;VRROptimizeEnable=1;".into()))));
    }

    #[test]
    fn registry_field_refuses_unexpected_types() {
        let (mut e, pf) = engine();
        let raw = RegData::Raw { ty: 3, data: vec![1, 2] };
        pf.with(|s| s.reg.insert(("HKCU\\Software\\Microsoft\\DirectX\\UserGpuPreferences".into(), "DirectXUserGlobalSettings".into()), raw.clone()));
        assert!(e.apply("windowed_flip_model").is_err());
        pf.with(|s| assert_eq!(s.reg.values().next(), Some(&raw)));
        assert!(e.journal.entries.is_empty());
    }

    #[test]
    fn kv_helpers() {
        assert_eq!(get_kv("A=1; SwapEffectUpgradeEnable=0;", "swapeffectupgradeenable").as_deref(), Some("0"));
        assert_eq!(get_kv("A=1;", "B"), None);
        assert_eq!(remove_kv("A=1;B=2;", "a"), "B=2;");
    }

    #[test]
    fn windows_processes_are_never_lowered() {
        let pf = MockPlatform::new();
        pf.with(|s| {
            s.procs = vec![
                (crate::platform::ProcInfo { pid: 1, exe_name: "dwm.exe".into() }, Priority::Normal),
                (crate::platform::ProcInfo { pid: 2, exe_name: "chrome.exe".into() }, Priority::Normal),
            ]
        });
        let a = Action::BackgroundPriority { class: Priority::BelowNormal, processes: vec!["dwm.exe".into(), "chrome.exe".into()] };
        let ctx = GameCtx { pid: 99, exe_path: "game.exe" };
        apply_action_recorded(&pf, &a, Some(ctx), &mut |_| Ok(())).unwrap();
        pf.with(|s| {
            assert_eq!(s.procs[0].1, Priority::Normal);
            assert_eq!(s.procs[1].1, Priority::BelowNormal);
        });
    }
}
