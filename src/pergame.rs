//! Per-game changes that must outlive a session because Windows reads them when the game
//! starts: graphics preference and the launch priority used for anti-cheat protected games.
//! Each records the real previous value; `Restore all` and turning the tweak off undo them.

use crate::catalog::RegData;
use crate::engine::{ifeo_key, ifeo_priority, revert_prior, undo_failed, Prior};
use crate::catalog::Priority;
use crate::platform::{file_name_lower, PResult, Platform};
use crate::store::Store;
use serde::{Deserialize, Serialize};

const FILE: &str = "per_game.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerGameEntry {
    pub tweak: String,
    /// Lower-case exe file name.
    pub exe: String,
    pub prior: Prior,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PerGame {
    pub entries: Vec<PerGameEntry>,
    #[serde(skip)]
    store: Option<Store>,
}

impl PerGame {
    pub fn load(store: &Store) -> PerGame {
        let mut p: PerGame = store.load(FILE).unwrap_or_default();
        p.store = Some(store.clone());
        p
    }

    fn save(&self) -> PResult<()> {
        match &self.store {
            Some(s) => s.save(FILE, self),
            None => Ok(()),
        }
    }

    pub fn has(&self, tweak: &str, exe_path: &str) -> bool {
        let exe = file_name_lower(exe_path);
        self.entries.iter().any(|e| e.tweak == tweak && e.exe == exe)
    }

    /// Persist a prior before its change runs. On error nothing is kept and the caller must
    /// not make the change.
    pub fn record(&mut self, tweak: &str, exe_path: &str, prior: Prior) -> PResult<()> {
        self.entries.push(PerGameEntry { tweak: tweak.into(), exe: file_name_lower(exe_path), prior });
        let r = self.save();
        if r.is_err() {
            self.entries.pop();
        }
        r
    }

    /// The change after [`record`](Self::record) failed: undo it, and drop the record if that worked.
    pub fn forget_failed(&mut self, pf: &dyn Platform, tweak: &str, exe_path: &str) {
        let exe = file_name_lower(exe_path);
        let Some(i) = self.entries.iter().rposition(|e| e.tweak == tweak && e.exe == exe) else { return };
        if undo_failed(pf, &self.entries[i].prior) {
            self.entries.remove(i);
            let _ = self.save();
        }
    }

    /// Fallback when Windows refuses to change a running game's priority (anti-cheat
    /// protection): set the launch priority so it applies from the next start.
    pub fn launch_priority(&mut self, pf: &dyn Platform, tweak: &str, exe_path: &str, class: Priority) -> PResult<bool> {
        if self.has(tweak, exe_path) {
            return Ok(false);
        }
        let key = ifeo_key(&file_name_lower(exe_path));
        let before = pf.reg_get(&key, "CpuPriorityClass")?;
        self.record(tweak, exe_path, Prior::Registry { key: key.clone(), name: "CpuPriorityClass".into(), value: before })?;
        if let Err(e) = pf.reg_set(&key, "CpuPriorityClass", &RegData::Dword(ifeo_priority(class))) {
            self.forget_failed(pf, tweak, exe_path);
            return Err(e);
        }
        Ok(true)
    }

    fn revert_where(&mut self, pf: &dyn Platform, keep: impl Fn(&PerGameEntry) -> bool) -> Vec<String> {
        let mut errs = Vec::new();
        let mut left = Vec::new();
        for e in std::mem::take(&mut self.entries).into_iter().rev() {
            if keep(&e) {
                left.push(e);
            } else if let Err(err) = revert_prior(pf, &e.prior) {
                errs.push(format!("{} ({}): {err}", e.tweak, e.exe));
                left.push(e);
            }
        }
        left.reverse();
        self.entries = left;
        if let Err(e) = self.save() {
            errs.push(format!("could not update {FILE}: {e}"));
        }
        errs
    }

    pub fn revert_tweak(&mut self, pf: &dyn Platform, tweak: &str) -> Vec<String> {
        self.revert_where(pf, |e| e.tweak != tweak)
    }

    pub fn revert_all(&mut self, pf: &dyn Platform) -> Vec<String> {
        self.revert_where(pf, |_| false)
    }
}
