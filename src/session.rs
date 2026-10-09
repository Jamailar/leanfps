//! A game session: session tweaks applied when a game starts and reverted when it exits.
//! Priors are saved to `session.json` as they are applied, so a crash or power cut is
//! repaired on the next start ([`recover`]).

use crate::catalog::{Action, Catalog, Scope};
use crate::config::Config;
use crate::engine::{apply_action, revert_prior, Entry, GameCtx};
use crate::pergame::PerGame;
use crate::catalog::Priority;
use crate::games::GameInfo;
use crate::platform::Platform;
use crate::store::Store;
use std::time::{Duration, Instant};

const SESSION_FILE: &str = "session.json";
const PURGE_COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub tweak: String,
    pub title: String,
    pub genre_specific: bool,
    pub result: Result<(), String>,
    /// Extra context, e.g. "applies from the next launch".
    pub note: Option<String>,
}

pub struct Session {
    pub game: GameInfo,
    pub pid: u32,
    pub started: Instant,
    pub applied: Vec<Applied>,
    priors: Vec<Entry>,
    purge_below_mb: Option<u64>,
    last_purge: Option<Instant>,
    pub purges: u32,
}

impl Session {
    pub fn start(pf: &dyn Platform, catalog: &Catalog, cfg: &Config, store: &Store, per_game: &mut PerGame, game: GameInfo, pid: u32) -> Session {
        let mut s = Session {
            game,
            pid,
            started: Instant::now(),
            applied: Vec::new(),
            priors: Vec::new(),
            purge_below_mb: None,
            last_purge: None,
            purges: 0,
        };
        for t in catalog.tweak.iter().filter(|t| t.scope == Scope::Session) {
            if !cfg.active(t) || !t.applies_to_genre(s.game.genre) {
                continue;
            }
            let mut result = Ok(());
            let mut note = None;
            let exe = s.game.exe_path.clone();
            let ctx = GameCtx { pid, exe_path: &exe };
            for a in &t.actions {
                if let Action::StandbyPurge { below_free_mb } = a {
                    s.purge_below_mb = Some(*below_free_mb);
                }
                if a.is_sticky() {
                    if per_game.has(&t.id, &exe) {
                        continue;
                    }
                    match apply_action(pf, a, Some(ctx)) {
                        Ok(prior) => {
                            per_game.record(&t.id, &exe, prior);
                            note = Some("Takes effect the next time the game starts".to_string());
                        }
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                    continue;
                }
                match apply_action(pf, a, Some(ctx)) {
                    Ok(prior) => {
                        s.priors.push(Entry { tweak: t.id.clone(), prior });
                        let _ = store.save(SESSION_FILE, &s.priors);
                    }
                    // Anti-cheat protected games refuse priority changes while running; Windows
                    // can still apply a launch priority to them.
                    Err(e) if matches!(a, Action::GamePriority { class } if *class >= Priority::AboveNormal) => {
                        let Action::GamePriority { class } = a else { unreachable!() };
                        match per_game.launch_priority(pf, &t.id, &exe, *class) {
                            Ok(true) => note = Some("Protected game: set for its next launch".to_string()),
                            Ok(false) => note = Some("Protected game: Windows sets this at launch".to_string()),
                            Err(_) => {
                                result = Err(e);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            s.applied.push(Applied { tweak: t.id.clone(), title: t.title.clone(), genre_specific: t.is_genre_specific(), result, note });
        }
        s
    }

    /// Called every couple of seconds while the game runs.
    pub fn tick(&mut self, pf: &dyn Platform) {
        let Some(limit) = self.purge_below_mb else { return };
        if self.last_purge.is_some_and(|t| t.elapsed() < PURGE_COOLDOWN) {
            return;
        }
        if pf.free_memory_mb().is_ok_and(|free| free < limit) && pf.purge_standby_list().is_ok() {
            self.last_purge = Some(Instant::now());
            self.purges += 1;
        }
    }

    pub fn alive(&self, pf: &dyn Platform) -> bool {
        pf.is_alive(self.pid)
    }

    /// Revert everything, newest first. Returns errors (already logged nowhere else).
    pub fn end(self, pf: &dyn Platform, store: &Store) -> Vec<String> {
        let errs = revert_all(pf, &self.priors);
        store.remove(SESSION_FILE);
        errs
    }
}

fn revert_all(pf: &dyn Platform, priors: &[Entry]) -> Vec<String> {
    priors.iter().rev().filter_map(|e| revert_prior(pf, &e.prior).err().map(|err| format!("{}: {err}", e.tweak))).collect()
}

/// Undo a session left behind by a crash. Safe to call when there is none.
pub fn recover(pf: &dyn Platform, store: &Store) -> Vec<String> {
    let Some(priors) = store.load::<Vec<Entry>>(SESSION_FILE) else { return Vec::new() };
    let errs = revert_all(pf, &priors);
    store.remove(SESSION_FILE);
    errs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Genre, Priority};
    use crate::games::Source;
    use crate::platform::mock::MockPlatform;
    use crate::platform::{Platform, ProcInfo};

    fn setup() -> (MockPlatform, Catalog, Config, Store) {
        let pf = MockPlatform::new();
        pf.with(|s| {
            s.procs = vec![
                (ProcInfo { pid: 1, exe_name: "chrome.exe".into() }, Priority::Normal),
                (ProcInfo { pid: 2, exe_name: "cs2.exe".into() }, Priority::Normal),
                (ProcInfo { pid: 3, exe_name: "Discord.exe".into() }, Priority::Normal),
            ];
        });
        (pf, Catalog::builtin(), Config::default(), Store::temp())
    }

    fn game(genre: Option<Genre>) -> GameInfo {
        GameInfo { name: "CS2".into(), genre, exe_path: "cs2.exe".into(), source: Source::Known }
    }

    #[test]
    fn shooter_session_applies_and_reverts() {
        let (pf, cat, cfg, store) = setup();
        let s = Session::start(&pf, &cat, &cfg, &store, &mut PerGame::load(&store), game(Some(Genre::Shooter)), 2);
        assert!(s.applied.iter().all(|a| a.result.is_ok()), "{:?}", s.applied);
        assert!(s.applied.iter().any(|a| a.tweak == "shooter_mouse_accel_off" && a.genre_specific));
        assert!(!s.applied.iter().any(|a| a.tweak == "standby_purge"), "open-world tweak not for shooters");
        pf.with(|st| {
            assert_eq!(st.procs[1].1, Priority::High);
            assert_eq!(st.procs[0].1, Priority::BelowNormal, "browser lowered");
            assert_eq!(st.procs[2].1, Priority::Normal, "voice chat untouched");
            assert_eq!(st.mouse, [0, 0, 0]);
            assert!(st.keep_awake);
        });
        assert!(store.path("session.json").exists());
        let errs = s.end(&pf, &store);
        assert!(errs.is_empty(), "{errs:?}");
        pf.with(|st| {
            assert!(st.procs.iter().all(|p| p.1 == Priority::Normal));
            assert_eq!(st.mouse, [6, 10, 1]);
            assert!(!st.keep_awake);
        });
        assert!(!store.path("session.json").exists());
    }

    #[test]
    fn crash_recovery_reverts_left_over_session() {
        let (pf, cat, cfg, store) = setup();
        let s = Session::start(&pf, &cat, &cfg, &store, &mut PerGame::load(&store), game(Some(Genre::Shooter)), 2);
        std::mem::forget(s); // simulate a crash: end() never runs
        assert_eq!(pf.with(|st| st.mouse), [0, 0, 0]);
        assert!(recover(&pf, &store).is_empty());
        pf.with(|st| {
            assert_eq!(st.mouse, [6, 10, 1]);
            assert!(!st.keep_awake);
        });
        assert!(recover(&pf, &store).is_empty(), "second call is a no-op");
    }

    #[test]
    fn free_tier_only_runs_free_session_tweaks() {
        let (pf, cat, mut cfg, store) = setup();
        cfg.sessions_played = 10;
        let s = Session::start(&pf, &cat, &cfg, &store, &mut PerGame::load(&store), game(Some(Genre::Shooter)), 2);
        let ids: Vec<_> = s.applied.iter().map(|a| a.tweak.as_str()).collect();
        assert_eq!(ids, vec!["keep_awake"]);
        s.end(&pf, &store);
    }

    #[test]
    fn protected_game_gets_launch_priority_instead() {
        let (pf, cat, cfg, store) = setup();
        pf.with(|st| st.fail.push("set_priority:2".into()));
        let mut pg = PerGame::load(&store);
        let s = Session::start(&pf, &cat, &cfg, &store, &mut pg, game(Some(Genre::Shooter)), 2);
        let prio = s.applied.iter().find(|a| a.tweak == "competitive_priority_high").unwrap();
        assert!(prio.result.is_ok() && prio.note.is_some(), "{prio:?}");
        let key = crate::engine::ifeo_key("cs2.exe");
        assert_eq!(pf.reg_get(&key, "CpuPriorityClass").unwrap(), Some(crate::catalog::RegData::Dword(3)));
        assert!(s.applied.iter().any(|a| a.tweak == "keep_awake" && a.result.is_ok()));
        assert!(s.end(&pf, &store).is_empty());
        // Launch priority stays after the session; Restore all removes it.
        assert!(pf.reg_get(&key, "CpuPriorityClass").unwrap().is_some());
        assert!(PerGame::load(&store).revert_all(&pf).is_empty());
        assert_eq!(pf.reg_get(&key, "CpuPriorityClass").unwrap(), None);
    }

    #[test]
    fn laptop_session_sets_power_mode_and_gpu_preference() {
        let (pf, cat, cfg, store) = setup();
        pf.with(|st| st.machine.laptop = true);
        let mut pg = PerGame::load(&store);
        let g = GameInfo { name: "X".into(), genre: Some(Genre::OpenWorld), exe_path: r"C:\\Games\\x.exe".into(), source: Source::Known };
        let s = Session::start(&pf, &cat, &cfg, &store, &mut pg, g.clone(), 2);
        assert!(s.applied.iter().all(|a| a.result.is_ok()), "{:?}", s.applied);
        assert_eq!(pf.power_overlay().unwrap(), "ded574b5-45a0-4f42-8737-46345c09c238");
        let pref = pf.reg_get(crate::engine::GPU_PREFS_KEY, r"C:\\Games\\x.exe").unwrap();
        assert_eq!(pref, Some(crate::catalog::RegData::Sz("GpuPreference=2;".into())));
        s.end(&pf, &store);
        assert_eq!(pf.power_overlay().unwrap(), "00000000-0000-0000-0000-000000000000", "power mode restored");
        // Second session: GPU preference already in place, not recorded twice.
        let s = Session::start(&pf, &cat, &cfg, &store, &mut pg, g, 2);
        s.end(&pf, &store);
        assert_eq!(pg.entries.len(), 1);
        assert!(pg.revert_all(&pf).is_empty());
        assert_eq!(pf.reg_get(crate::engine::GPU_PREFS_KEY, r"C:\\Games\\x.exe").unwrap(), None);
    }

    #[test]
    fn open_world_purges_standby_when_memory_low() {
        let (pf, cat, cfg, store) = setup();
        let mut s = Session::start(&pf, &cat, &cfg, &store, &mut PerGame::load(&store), game(Some(Genre::OpenWorld)), 2);
        s.tick(&pf);
        assert_eq!(s.purges, 0);
        pf.with(|st| st.free_mb = 500);
        s.tick(&pf);
        s.tick(&pf);
        assert_eq!(s.purges, 1, "cooldown between purges");
        s.end(&pf, &store);
    }
}
