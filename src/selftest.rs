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
        Action::Registry { key, name, .. } | Action::RegistryField { key, name, .. } => Snap::Reg(pf.reg_get(key, name).unwrap_or(None)),
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

/// Only on CI or when explicitly allowed: the selftest changes real settings and keeps its
/// undo journal in a temp folder that `--restore-all` doesn't know about.
fn allowed() -> bool {
    std::env::var("CI").is_ok_and(|v| v.eq_ignore_ascii_case("true")) || std::env::var("LEANFPS_SELFTEST").is_ok_and(|v| v == "1")
}

pub fn run() -> i32 {
    if !allowed() {
        println!("--selftest changes real system settings and is meant for throwaway test machines.");
        println!("It only runs on CI, or with the environment variable LEANFPS_SELFTEST=1.");
        return 2;
    }
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
            // CPU sets (V-Cache / performance-core tweaks): set, read back, clear.
            let sets = pf.cpu_sets();
            out!(log, "cpu sets: {} logical processors, L3 sizes {:?} KB, efficiency classes {:?}", sets.len(),
                sets.iter().map(|s| s.l3_kb).collect::<std::collections::BTreeSet<_>>(), sets.iter().map(|s| s.efficiency).collect::<std::collections::BTreeSet<_>>());
            if sets.len() >= 2 {
                let want: Vec<u32> = sets.iter().take(sets.len() / 2).map(|s| s.id).collect();
                let before = pf.process_cpu_sets(pid);
                let set = pf.set_process_cpu_sets(pid, &want);
                let during = pf.process_cpu_sets(pid);
                let clear = pf.set_process_cpu_sets(pid, &[]);
                let after = pf.process_cpu_sets(pid);
                if before == Ok(vec![]) && set.is_ok() && during.as_ref() == Ok(&want) && clear.is_ok() && after == Ok(vec![]) {
                    out!(log, "OK   cpu sets set, read back and cleared");
                } else {
                    failures += 1;
                    out!(log, "FAIL cpu sets: before={before:?} set={set:?} during={during:?} clear={clear:?} after={after:?}");
                }
            } else {
                failures += 1;
                out!(log, "FAIL no CPU set information");
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

    out!(log, "\n== frame counter (300 presents from a Direct3D 11 WARP window)");
    #[cfg(windows)]
    {
        use crate::frames::Sources;
        // DirectX events, then kernel events alone: the path Vulkan and OpenGL games are counted by.
        for (label, sources) in [("DXGI events", Sources { runtime: true, kernel: false }), ("kernel events", Sources { runtime: false, kernel: true }), ("both", Sources::default())] {
            match present_test(sources) {
                Ok((frames, avg)) if (250..=310).contains(&frames) => out!(log, "OK   {label}: counted {frames} frame times, {avg:.0} FPS average"),
                Ok((frames, avg)) => {
                    failures += 1;
                    out!(log, "FAIL {label}: counted {frames} frame times ({avg:.0} FPS), expected about 299");
                }
                Err(e) => {
                    failures += 1;
                    out!(log, "FAIL {label}: {e}");
                }
            }
        }
    }

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
        let full_us = t.elapsed().as_micros() / n as u128;
        // The watcher judges a process once (cached 10 s); the per-tick cost is reading the
        // foreground window.
        let t = Instant::now();
        for _ in 0..n {
            let _ = pf.foreground();
        }
        let tick_us = t.elapsed().as_micros() / n as u128;
        out!(log, "first look at a new foreground app (worst case, includes module scan): {full_us} us");
        let (cpu1, mem) = pf.self_usage();
        out!(log, "steady watcher tick (every 2 s): {tick_us} us; CPU used by this whole test: {} ms", cpu1.saturating_sub(cpu0));
        out!(log, "working set: {mem} MB");
        if tick_us > 2000 || full_us > 20_000 {
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

/// Present 300 frames from this process and count them with the ETW frame counter.
#[cfg(windows)]
fn present_test(sources: crate::frames::Sources) -> Result<(u64, f64), String> {
    use ::windows::core::{w, HSTRING};
    use ::windows::Win32::Foundation::{HMODULE, HWND};
    use ::windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
    use ::windows::Win32::Graphics::Direct3D11::{D3D11CreateDeviceAndSwapChain, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION};
    use ::windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_SAMPLE_DESC};
    use ::windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT};
    use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use ::windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, ShowWindow, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };

    unsafe extern "system" fn wndproc(h: HWND, m: u32, w: ::windows::Win32::Foundation::WPARAM, l: ::windows::Win32::Foundation::LPARAM) -> ::windows::Win32::Foundation::LRESULT {
        DefWindowProcW(h, m, w, l)
    }
    let capture = crate::frames::PresentCapture::start_with(std::process::id(), sources)?;
    unsafe {
        let inst = GetModuleHandleW(None).map_err(|e| e.message().to_string())?;
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: w!("LeanFPS.PresentTest"), ..Default::default() };
        RegisterClassW(&wc);
        let hwnd: HWND = CreateWindowExW(WINDOW_EX_STYLE::default(), w!("LeanFPS.PresentTest"), &HSTRING::from("present test"), WS_OVERLAPPEDWINDOW, 0, 0, 320, 240, None, None, Some(inst.into()), None)
            .map_err(|e| e.message().to_string())?;
        // Visible, so the presents really reach the screen (and the kernel present path).
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC { Width: 320, Height: 240, Format: DXGI_FORMAT_R8G8B8A8_UNORM, ..Default::default() },
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 1,
            OutputWindow: hwnd,
            Windowed: true.into(),
            SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
            Flags: 0,
        };
        let mut sc: Option<IDXGISwapChain> = None;
        D3D11CreateDeviceAndSwapChain(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), None, D3D11_SDK_VERSION, Some(&desc), Some(&mut sc), None, None, None)
            .map_err(|e| format!("could not create a Direct3D 11 device: {}", e.message()))?;
        let sc = sc.ok_or("no swap chain")?;
        // Give the ETW session a moment to start delivering events.
        std::thread::sleep(std::time::Duration::from_millis(1500));
        for _ in 0..300 {
            let _ = sc.Present(0, DXGI_PRESENT(0));
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        drop(sc);
        let _ = DestroyWindow(hwnd);
    }
    // Real-time sessions flush about once a second.
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let st = capture.finish();
    Ok((st.frames, st.avg_fps()))
}
