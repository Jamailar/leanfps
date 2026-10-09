#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod catalog;
mod config;
mod engine;
mod games;
mod license;
mod pergame;
mod platform;
mod selftest;
mod session;
mod store;
mod ui;
mod watcher;

use std::sync::{mpsc, Arc, Mutex};

const HELP: &str = "LeanFPS - game performance optimizer

  leanfps              open the window
  leanfps --restore-all  undo every change LeanFPS made, then exit
  leanfps --selftest     apply, verify and undo every tweak (test machines only)
  leanfps --version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        attach_console();
    }
    match args.first().map(String::as_str) {
        None => run_window(),
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

fn run_window() {
    #[cfg(windows)]
    if !platform::win::single_instance() {
        platform::win::focus_existing_window();
        return;
    }
    let pf: Arc<dyn platform::Platform> = Arc::from(platform::native());
    let store = store::Store::default_location();
    let catalog = catalog::Catalog::builtin();
    let config = Arc::new(Mutex::new(store.load::<config::Config>("config.json").unwrap_or_default()));
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
            Ok(Box::new(ui::App::new(cc, engine, games::GameDb::builtin(), config, shared, store, per_game, tx)))
        }),
    );
    if let Err(e) = result {
        eprintln!("LeanFPS could not open its window: {e}");
    }
    let handle = watcher_thread.lock().unwrap().take();
    if let Some(h) = handle {
        let _ = h.join();
    }
}

fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use ::windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
