//! Background thread: watches the foreground window, starts a session when a game shows up
//! and ends it when the game exits. Cheap: one foreground check every 2 seconds.

use crate::catalog::Catalog;
use crate::config::{Config, Tier};
use crate::games::library::Installed;
use crate::games::{classify, GameDb, GameInfo, SteamGenres, Verdict};
use crate::platform::{Icon, Platform};
use crate::session::{self, Applied, Session};
use crate::store::Store;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const TICK: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct SessionView {
    pub game: GameInfo,
    pub icon: Option<Icon>,
    pub applied: Vec<Applied>,
    pub started: Instant,
    pub purges: u32,
    pub trial: bool,
    /// Frame counter: None when unavailable (not admin, unsupported API).
    pub fps: Option<Fps>,
    /// Earlier sessions of this game: (avg FPS, 1% low, sessions).
    pub usual: Option<(f64, f64, usize)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Fps {
    pub live: f64,
    pub avg: f64,
    pub low1: f64,
    pub frames: u64,
}

/// What the last finished session measured, for the "last game" line.
#[derive(Debug, Clone)]
pub struct LastGame {
    pub record: crate::frames::Record,
    pub usual: Option<(f64, f64, usize)>,
}

#[derive(Default)]
pub struct Shared {
    pub session: Option<SessionView>,
    pub installed: Vec<Installed>,
    /// A windowed 3D app that may be a game; the UI asks the user.
    pub candidate: Option<GameInfo>,
    pub last_game: Option<LastGame>,
    /// Problems the user should see (e.g. leftovers that could not be restored).
    pub notices: Vec<String>,
    /// Bumped whenever something changes, so the UI knows to redraw.
    pub version: u64,
}

pub enum Command {
    /// Settings changed: end the current session; the next tick starts it again with the new settings.
    Restart,
    /// App is closing: revert and stop.
    Stop,
}

pub struct Watcher {
    pub pf: Arc<dyn Platform>,
    pub catalog: Catalog,
    pub db: GameDb,
    pub tags: Arc<SteamGenres>,
    pub config: Arc<Mutex<Config>>,
    pub shared: Arc<Mutex<Shared>>,
    pub store: Store,
    pub per_game: Arc<Mutex<crate::pergame::PerGame>>,
    pub on_change: Box<dyn Fn() + Send>,
}

impl Watcher {
    pub fn run(self, rx: Receiver<Command>) {
        let errs = session::recover(&*self.pf, &self.store);
        self.update(|s| {
            s.notices.extend(errs.into_iter().map(|e| format!("Could not restore after last crash: {e}")));
        });
        let mut current: Option<Session> = None;
        let mut capture: Option<Capture> = None;
        // Trial games are counted once per game process, not again when settings restart the session.
        let mut counted: Option<u32> = None;
        // Foreground process already judged not to be a game, and when: skip re-checking it
        // every tick (re-check every 10 s in case it went fullscreen).
        let mut judged: Option<(u32, Instant)> = None;
        loop {
            match rx.recv_timeout(TICK) {
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    if let Some(s) = current.take() {
                        self.finish_capture(&s, capture.take());
                        s.end(&*self.pf, &self.store);
                    }
                    self.update(|s| s.session = None);
                    return;
                }
                Ok(Command::Restart) => {
                    judged = None;
                    if let Some(s) = current.take() {
                        self.finish_capture(&s, capture.take());
                        s.end(&*self.pf, &self.store);
                    }
                    self.update(|s| s.session = None);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick(&mut current, &mut capture, &mut counted, &mut judged);
        }
    }

    fn update(&self, f: impl FnOnce(&mut Shared)) {
        {
            let mut s = self.shared.lock().unwrap();
            f(&mut s);
            s.version += 1;
        }
        (self.on_change)();
    }

    fn tick(&self, current: &mut Option<Session>, capture: &mut Option<Capture>, counted: &mut Option<u32>, judged: &mut Option<(u32, Instant)>) {
        if let Some(s) = current.as_mut() {
            if s.alive(&*self.pf) {
                s.tick(&*self.pf);
                let n = s.purges;
                let fps = capture.as_ref().map(capture_fps);
                self.update(|sh| {
                    if let Some(v) = sh.session.as_mut() {
                        v.purges = n;
                        v.fps = fps;
                    }
                });
                return;
            }
            let s = current.take().unwrap();
            self.finish_capture(&s, capture.take());
            let errs = s.end(&*self.pf, &self.store);
            self.update(|sh| {
                sh.session = None;
                sh.notices.extend(errs);
            });
            return;
        }

        let Some(fg) = self.pf.foreground() else { return };
        if judged.is_some_and(|(pid, at)| pid == fg.pid && at.elapsed() < Duration::from_secs(10)) {
            return;
        }
        let (verdict, trial) = {
            let cfg = self.config.lock().unwrap();
            let shared = self.shared.lock().unwrap();
            let pf = &self.pf;
            let pid = fg.pid;
            let v = classify(&fg.exe_path, fg.fullscreen, fg.coverage, &|| pf.uses_graphics_api(pid), &self.db, &self.tags, &shared.installed, &cfg.prefs());
            (v, matches!(cfg.tier(), Tier::Trial { .. }))
        };
        let game = match verdict {
            Verdict::Game(g) => g,
            Verdict::Ask(g) => {
                *judged = Some((fg.pid, Instant::now()));
                let changed = self.shared.lock().unwrap().candidate.as_ref().map(|c| &c.exe_path) != Some(&g.exe_path);
                if changed {
                    self.update(|sh| sh.candidate = Some(g));
                }
                return;
            }
            Verdict::NotGame => {
                *judged = Some((fg.pid, Instant::now()));
                return;
            }
        };
        if self.shared.lock().unwrap().candidate.is_some() {
            self.update(|sh| sh.candidate = None);
        }
        let cfg_snapshot = self.config.lock().unwrap().clone();
        let s = Session::start(&*self.pf, &self.catalog, &cfg_snapshot, &self.store, &mut self.per_game.lock().unwrap(), game.clone(), fg.pid);
        if trial && *counted != Some(fg.pid) {
            *counted = Some(fg.pid);
            let mut cfg = self.config.lock().unwrap();
            cfg.sessions_played += 1;
            let _ = self.store.save("config.json", &*cfg);
        }
        let icon = self.pf.exe_icon(&game.exe_path, 64);
        *capture = start_capture(fg.pid);
        let history: Vec<crate::frames::Record> = self.store.load(HISTORY).unwrap_or_default();
        let usual = crate::frames::usual(&history, &crate::platform::file_name_lower(&game.exe_path));
        let fps = capture.as_ref().map(capture_fps);
        let view = SessionView { game, icon, applied: s.applied.clone(), started: s.started, purges: 0, trial, fps, usual };
        *current = Some(s);
        self.update(|sh| sh.session = Some(view));
    }
}

const HISTORY: &str = "history.json";
const HISTORY_MAX: usize = 500;

#[cfg(windows)]
type Capture = crate::frames::PresentCapture;
#[cfg(not(windows))]
type Capture = ();

#[cfg(windows)]
fn start_capture(pid: u32) -> Option<Capture> {
    crate::frames::PresentCapture::start(pid).ok()
}
#[cfg(not(windows))]
fn start_capture(_pid: u32) -> Option<Capture> {
    // UI work off Windows: LEANFPS_DEMO_FPS=1 shows made-up frame numbers.
    std::env::var_os("LEANFPS_DEMO_FPS").map(|_| ())
}

#[cfg(windows)]
fn capture_fps(c: &Capture) -> Fps {
    let st = c.stats();
    Fps { live: c.live_fps(), avg: st.avg_fps(), low1: st.low_fps(1.0), frames: st.frames }
}
#[cfg(not(windows))]
fn capture_fps(_c: &Capture) -> Fps {
    Fps { live: 241.0, avg: 236.0, low1: 168.0, frames: 4000 }
}

impl Watcher {
    /// Stop counting frames and keep the session in the history if it was long enough.
    fn finish_capture(&self, s: &Session, capture: Option<Capture>) {
        let Some(c) = capture else { return };
        #[cfg(windows)]
        let stats = c.finish();
        #[cfg(not(windows))]
        let stats = {
            let _ = c;
            crate::frames::FrameStats::default()
        };
        if stats.frames < 600 {
            return;
        }
        let exe = crate::platform::file_name_lower(&s.game.exe_path);
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(s.started.elapsed().as_secs());
        let ok = s.applied.iter().filter(|a| a.result.is_ok()).count();
        let record = crate::frames::Record::from_stats(&exe, &s.game.name, started, &stats, ok);
        let mut history: Vec<crate::frames::Record> = self.store.load(HISTORY).unwrap_or_default();
        let usual = crate::frames::usual(&history, &exe);
        history.push(record.clone());
        if history.len() > HISTORY_MAX {
            let extra = history.len() - HISTORY_MAX;
            history.drain(..extra);
        }
        let _ = self.store.save(HISTORY, &history);
        self.update(|sh| sh.last_game = Some(LastGame { record, usual }));
    }
}

/// Installed games on this PC: Steam, Epic, GOG, Battle.net, Ubisoft.
pub fn scan_installed() -> Vec<Installed> {
    #[cfg(windows)]
    {
        use crate::platform::{uninstall_entries, win, Platform};
        let pf = win::WinPlatform::new();
        let uninstall = uninstall_entries(&pf);
        let ubi_root = "HKLM\\SOFTWARE\\WOW6432Node\\Ubisoft\\Launcher\\Installs";
        let ubisoft: Vec<String> = pf
            .reg_subkeys(ubi_root)
            .iter()
            .filter_map(|id| match pf.reg_get(&format!("{ubi_root}\\{id}"), "InstallDir") {
                Ok(Some(crate::catalog::RegData::Sz(d))) => Some(d),
                _ => None,
            })
            .collect();
        crate::games::library::scan(win::steam_root().as_deref(), win::epic_manifests().as_deref(), &uninstall, &ubisoft)
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
