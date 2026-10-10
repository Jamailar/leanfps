#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod catalog;
mod config;
mod engine;
mod frames;
mod games;
mod license;
mod pergame;
mod platform;
mod selftest;
mod session;
mod store;
mod tray;
mod ui;
mod watcher;

use std::sync::{mpsc, Arc, Mutex};

const HELP: &str = "LeanFPS - game performance optimizer

  leanfps              open the window
  leanfps --tray       start hidden in the tray (used by Start with Windows)
  leanfps --restore-all  undo every change LeanFPS made, then exit
  leanfps --selftest     apply, verify and undo every tweak (test machines only)
  leanfps --version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        attach_console();
    }
    // A windowed release build has no console, so a panic or a failed window would vanish
    // without a trace. Keep a short log in the data folder and tell the user where it is.
    let windowed = matches!(args.first().map(String::as_str), None | Some("--tray"));
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("crashed: {info}");
        log_line(&msg);
        eprintln!("{msg}");
        if windowed {
            fatal_box(&msg);
        }
    }));
    log_line(&format!("LeanFPS {} started {:?}", env!("CARGO_PKG_VERSION"), args));
    match args.first().map(String::as_str) {
        None => run_window(false),
        Some("--tray") => run_window(true),
        Some("--selftest") => std::process::exit(selftest::run()),
        Some("--restore-all") => std::process::exit(restore_all()),
        Some("--version") => println!("LeanFPS {}", env!("CARGO_PKG_VERSION")),
        Some(_) => println!("{HELP}"),
    }
}

fn restore_all() -> i32 {
    let pf: Arc<dyn platform::Platform> = Arc::from(platform::native());
    let store = store::Store::default_location();
    let session_errs = session::recover(&*pf, &store);
    let mut engine = engine::Engine::new(catalog::Catalog::builtin(), pf, store.clone());
    #[cfg(windows)]
    {
        let _ = platform::win::set_autostart(false);
        frames::stop_leftover();
    }
    let mut errs = engine.revert_all();
    errs.extend(pergame::PerGame::load(&store).revert_all(&*engine.pf).into_iter().map(|e| ("per-game".to_string(), e)));
    let mut cfg: config::Config = store.load("config.json").unwrap_or_default();
    cfg.set_preset("none");
    let _ = store.save("config.json", &cfg);
    for e in &session_errs {
        println!("could not restore: {e}");
    }
    for (id, e) in &errs {
        println!("could not restore {id}: {e}");
    }
    if errs.is_empty() && session_errs.is_empty() {
        println!("Every change was undone.");
        0
    } else {
        1
    }
}

fn run_window(hidden: bool) {
    #[cfg(windows)]
    if !platform::win::single_instance() {
        let woke = tray::wake_existing() || platform::win::focus_existing_window();
        log_line(&format!("already running; asked it to show its window: {woke}"));
        return;
    }
    let pf: Arc<dyn platform::Platform> = Arc::from(platform::native());
    let store = store::Store::default_location();
    let catalog = catalog::Catalog::builtin();
    let licenses = Arc::new(license::Licenses {
        store: store.clone(),
        transport: license::transport(),
        keys: license::PUBLIC_KEYS,
        device: license::device_info(&*pf),
        app_version: env!("CARGO_PKG_VERSION"),
        now: license::now_secs,
    });
    let mut cfg = store.load::<config::Config>("config.json").unwrap_or_default();
    // Checked offline from the signed file before the watcher starts, so the first game
    // already gets the right tweaks.
    cfg.pro = licenses.state().is_pro() || license::dev_pro();
    let config = Arc::new(Mutex::new(cfg));
    let shared = Arc::new(Mutex::new(watcher::Shared::default()));
    let engine = engine::Engine::new(catalog.clone(), pf.clone(), store.clone());
    let per_game = Arc::new(Mutex::new(pergame::PerGame::load(&store)));
    let (tx, rx) = mpsc::channel();
    let always_on_top = config.lock().unwrap().always_on_top;
    // The watcher must finish undoing session changes before the process exits.
    let watcher_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>> = Arc::new(Mutex::new(None));
    let watcher_slot = watcher_thread.clone();

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("LeanFPS")
        .with_inner_size(ui::WINDOW_SIZE)
        .with_min_inner_size([360.0, 480.0])
        .with_maximize_button(false)
        .with_icon(eframe::egui::IconData { rgba: include_bytes!("../res/window-64.rgba").to_vec(), width: 64, height: 64 });
    if hidden {
        viewport = viewport.with_visible(false);
    }
    if always_on_top {
        viewport = viewport.with_window_level(eframe::egui::WindowLevel::AlwaysOnTop);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    let result = eframe::run_native(
        "LeanFPS",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let w = watcher::Watcher {
                pf: pf.clone(),
                catalog: catalog.clone(),
                db: games::GameDb::builtin(),
                tags: Arc::new(games::SteamGenres::builtin()),
                config: config.clone(),
                shared: shared.clone(),
                store: store.clone(),
                per_game: per_game.clone(),
                on_change: Box::new(move || ctx.request_repaint()),
            };
            {
                let shared = shared.clone();
                let ctx = cc.egui_ctx.clone();
                std::thread::spawn(move || {
                    let installed = watcher::scan_installed();
                    let mut s = shared.lock().unwrap();
                    s.installed = installed;
                    s.version += 1;
                    drop(s);
                    ctx.request_repaint();
                });
            }
            let h = std::thread::Builder::new().name("watcher".into()).spawn(move || w.run(rx)).expect("start watcher");
            *watcher_slot.lock().unwrap() = Some(h);
            Ok(Box::new(ui::App::new(cc, engine, games::GameDb::builtin(), config, shared, store, per_game, tx, licenses)))
        }),
    );
    if let Err(e) = result {
        let msg = format!("LeanFPS could not open its window: {e}");
        log_line(&msg);
        eprintln!("{msg}");
        fatal_box(&msg);
    }
    let handle = watcher_thread.lock().unwrap().take();
    if let Some(h) = handle {
        let _ = h.join();
    }
}

/// Append one line to `leanfps.log` in the data folder (best effort, never fails).
fn log_line(msg: &str) {
    use std::io::Write;
    let path = store::Store::default_location().path("leanfps.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Keep the log small: start over once it passes 256 KB.
    if std::fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{secs}] {msg}");
    }
}

/// Show a startup failure to the user instead of exiting silently.
fn fatal_box(msg: &str) {
    #[cfg(windows)]
    {
        let log = store::Store::default_location().path("leanfps.log");
        platform::win::message_box("LeanFPS", &format!("{msg}\n\nDetails are in {}", log.display()));
    }
    #[cfg(not(windows))]
    let _ = msg;
}

fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use ::windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
