//! `leanfps --selftest`: apply, verify and revert every tweak on the real system, then check
//! that everything is exactly as before. Meant for a throwaway Windows VM (CI), run as admin.

use crate::catalog::{Action, Catalog, Genre, Priority, RegData, Scope};
use crate::config::Config;
use crate::engine::{Engine, TweakState};
use crate::games::{GameInfo, Source};
use crate::platform::Platform;
use crate::session::Session;
use crate::store::Store;
use std::io::Write;
use std::sync::Arc;

// Release builds are GUI-subsystem executables, so CI can't rely on stdout: every line also
// goes to `leanfps-selftest.log` in the current directory.
macro_rules! out {
    ($log:expr, $($arg:tt)*) => {{
        let line = format!($($arg)*);
        println!("{line}");
        let _ = writeln!($log, "{line}");
    }};
}

#[derive(Debug, PartialEq)]
enum Snap {
    Reg(Option<RegData>),
    Scheme(String),
    Power(String, (u32, u32)),
    Mouse([i32; 3]),
    None,
}

fn snapshot(pf: &dyn Platform, a: &Action) -> Snap {
    match a {
        Action::Registry { key, name, .. } => Snap::Reg(pf.reg_get(key, name).unwrap_or(None)),
        Action::PowerScheme { .. } => Snap::Scheme(pf.power_active_scheme().unwrap_or_default()),
        Action::PowerSetting { subgroup, setting, .. } => {
            let s = pf.power_active_scheme().unwrap_or_default();
            let v = pf.power_value(&s, subgroup, setting).unwrap_or((u32::MAX, u32::MAX));
            Snap::Power(s, v)
        }
        Action::MouseAcceleration { .. } => Snap::Mouse(pf.mouse_params().unwrap_or([-1; 3])),
        _ => Snap::None,
    }
}

pub fn run() -> i32 {
    let mut log = std::fs::File::create("leanfps-selftest.log").expect("create leanfps-selftest.log");
    let pf: Arc<dyn Platform> = Arc::from(crate::platform::native());
    let m = pf.machine();
    out!(log, "machine: laptop={} build={} cpu={}", m.laptop, m.os_build, m.cpu);
    let dir = std::env::temp_dir().join(format!("leanfps-selftest-{}", std::process::id()));
    let store = Store::new(dir.clone());
    let catalog = Catalog::builtin();
    let mut engine = Engine::new(catalog.clone(), pf.clone(), store.clone());
    let mut failures = 0;

    out!(log, "\n== persistent tweaks: apply -> verify -> revert -> compare");
    for t in catalog.tweak.iter().filter(|t| t.scope == Scope::Persistent) {
        if let TweakState::Unavailable(r) = engine.state(&t.id) {
            out!(log, "SKIP {:<28} {r}", t.id);
            continue;
        }
        let before: Vec<Snap> = t.actions.iter().map(|a| snapshot(&*pf, a)).collect();
        let res = engine.apply(&t.id).and_then(|_| match engine.state(&t.id) {
            TweakState::Applied => Ok(()),
            s => Err(format!("state after apply is {s:?}")),
        });
        let rev = engine.revert(&t.id);
        // Compare in reverse because actions may depend on each other (power plan).
        let after: Vec<Snap> = t.actions.iter().map(|a| snapshot(&*pf, a)).collect();
        match (res, rev, before == after) {
            (Ok(()), Ok(()), true) => out!(log, "OK   {}", t.id),
            (r, v, same) => {
                failures += 1;
                out!(log, "FAIL {} apply={r:?} revert={v:?} restored={same}", t.id);
                if !same {
                    out!(log, "     before={before:?}\n     after ={after:?}");
                }
            }
        }
    }

    out!(log, "\n== session tweaks against a stand-in game process");
    match std::process::Command::new("cmd").args(["/c", "ping -n 30 127.0.0.1 >NUL"]).spawn() {
        Err(e) => {
            failures += 1;
            out!(log, "FAIL could not start stand-in process: {e}");
        }
        Ok(mut child) => {
            let pid = child.id();
            let mut cfg = Config::default();
            cfg.set_preset("competitive");
            for genre in [Genre::Shooter, Genre::OpenWorld] {
                let prio_before = pf.priority(pid);
                let mouse_before = pf.mouse_params();
                let game = GameInfo { name: "Stand-in".into(), genre: Some(genre), exe_path: "cmd.exe".into(), source: Source::Known };
                let mut per_game = crate::pergame::PerGame::load(&store);
                let s = Session::start(&*pf, &catalog, &cfg, &store, &mut per_game, game, pid);
                for a in &s.applied {
                    match &a.result {
                        Ok(()) => out!(log, "OK   {:?} {}", genre, a.tweak),
                        Err(e) => {
                            failures += 1;
                            out!(log, "FAIL {:?} {} {e}", genre, a.tweak);
                        }
                    }
                }
                if s.applied.iter().any(|a| a.tweak.contains("priority_high")) && pf.priority(pid) != Ok(Priority::High) {
                    failures += 1;
                    out!(log, "FAIL priority not High during session: {:?}", pf.priority(pid));
                }
                let mut errs = s.end(&*pf, &store);
                errs.extend(per_game.revert_all(&*pf));
                if !errs.is_empty() || pf.priority(pid) != prio_before || pf.mouse_params() != mouse_before {
                    failures += 1;
                    out!(log, "FAIL {genre:?} not restored: errs={errs:?} prio={:?}/{prio_before:?} mouse={:?}/{mouse_before:?}", pf.priority(pid), pf.mouse_params());
                } else {
                    out!(log, "OK   {genre:?} session restored");
                }
            }
            let _ = child.kill();
        }
    }

    out!(log, "\n== detection");
    let installed = crate::watcher::scan_installed();
    out!(log, "installed games found: {}", installed.len());
    let icon = pf.exe_icon(r"C:\Windows\explorer.exe", 64);
    match icon {
        Some(i) if i.rgba.len() == 64 * 64 * 4 && i.rgba.chunks(4).any(|p| p[3] != 0) => out!(log, "OK   icon extraction"),
        other => {
            failures += 1;
            out!(log, "FAIL icon extraction: {:?}", other.map(|i| i.rgba.len()));
        }
    }
    out!(log, "free memory: {:?} MB", pf.free_memory_mb());

    out!(log, "\n== overhead (budgets: detection tick < 2 ms, memory < 40 MB without window)");
    {
        use crate::games::{classify, GameDb, Prefs, SteamGenres};
        use std::time::Instant;
        let t = Instant::now();
        let db = GameDb::builtin();
        let tags = SteamGenres::builtin();
        out!(log, "game databases loaded in {} ms ({} curated, {} Steam genres)", t.elapsed().as_millis(), db.game.len(), tags.len());
        let t = Instant::now();
        let installed = crate::watcher::scan_installed();
        out!(log, "installed-game scan: {} ms", t.elapsed().as_millis());
        let prefs = Prefs::default();
        let (cpu0, _) = pf.self_usage();
        let n = 500u32;
        let t = Instant::now();
        for _ in 0..n {
            // What the watcher does every 2 s: read the foreground window and judge it.
            match pf.foreground() {
                Some(fg) => {
                    let pid = fg.pid;
                    let _ = classify(&fg.exe_path, fg.fullscreen, fg.coverage, &|| pf.uses_graphics_api(pid), &db, &tags, &installed, &prefs);
                }
                None => {
                    let _ = classify(r"C:\Games\Some Game\game.exe", false, 0.5, &|| None, &db, &tags, &installed, &prefs);
                }
            }
        }
        let tick_us = t.elapsed().as_micros() / n as u128;
        let (cpu1, mem) = pf.self_usage();
        out!(log, "detection tick: {tick_us} us average over {n} ticks (CPU {} ms total)", cpu1.saturating_sub(cpu0));
        out!(log, "working set: {mem} MB");
        if tick_us > 2000 {
            failures += 1;
            out!(log, "FAIL detection tick over budget");
        }
        if mem > 40 {
            failures += 1;
            out!(log, "FAIL memory over budget");
        }
    }

    let _ = std::fs::remove_dir_all(dir);
    out!(log, "\n{} failure(s)", failures);
    if failures == 0 {
        0
    } else {
        1
    }
}
