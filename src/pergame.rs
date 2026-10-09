//! Per-game changes that must outlive a session because Windows reads them when the game
//! starts: graphics preference and the launch priority used for anti-cheat protected games.
//! Each records the real previous value; `Restore all` and turning the tweak off undo them.

use crate::catalog::RegData;
use crate::engine::{ifeo_key, ifeo_priority, revert_prior, Prior};
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

    fn save(&self) {
        if let Some(s) = &self.store {
            let _ = s.save(FILE, self);
        }
    }

    pub fn has(&self, tweak: &str, exe_path: &str) -> bool {
        let exe = file_name_lower(exe_path);
        self.entries.iter().any(|e| e.tweak == tweak && e.exe == exe)
    }

    pub fn record(&mut self, tweak: &str, exe_path: &str, prior: Prior) {
        self.entries.push(PerGameEntry { tweak: tweak.into(), exe: file_name_lower(exe_path), prior });
        self.save();
    }

    /// Fallback when Windows refuses to change a running game's priority (anti-cheat
    /// protection): set the launch priority so it applies from the next start.
    pub fn launch_priority(&mut self, pf: &dyn Platform, tweak: &str, exe_path: &str, class: Priority) -> PResult<bool> {
        if self.has(tweak, exe_path) {
            return Ok(false);
        }
        let key = ifeo_key(&file_name_lower(exe_path));
        let before = pf.reg_get(&key, "CpuPriorityClass")?;
        pf.reg_set(&key, "CpuPriorityClass", &RegData::Dword(ifeo_priority(class)))?;
        self.record(tweak, exe_path, Prior::Registry { key, name: "CpuPriorityClass".into(), value: before });
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
        self.save();
        errs
    }

    pub fn revert_tweak(&mut self, pf: &dyn Platform, tweak: &str) -> Vec<String> {
        self.revert_where(pf, |e| e.tweak != tweak)
    }

    pub fn revert_all(&mut self, pf: &dyn Platform) -> Vec<String> {
        self.revert_where(pf, |_| false)
    }
}
