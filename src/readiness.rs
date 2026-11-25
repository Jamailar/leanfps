//! Local read-only hardware and Windows readiness checks for demanding open-world games
//! (GTA V Enhanced, FiveM, Red Dead Redemption 2, and upcoming next-gen titles such as GTA 6 PC).
//! Never guesses FPS numbers and never modifies or weakens Windows security settings.

use crate::engine::{Engine, TweakState};
use crate::games::library::Installed;
use crate::platform::HardwareInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    Info,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: &'static str,
    pub title: String,
    pub status: Status,
    pub detail: String,
    pub action_tweak: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessReport {
    pub checks: Vec<Check>,
    pub ready_count: usize,
    pub total_checks: usize,
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn evaluate(hw: &HardwareInfo, engine: &Engine, installed: &[Installed]) -> ReadinessReport {
    evaluate_with(hw, installed, |id| engine.state(id))
}

pub fn evaluate_with(hw: &HardwareInfo, installed: &[Installed], state_of: impl Fn(&str) -> TweakState) -> ReadinessReport {
    let mut checks = Vec::new();

    // 1. GPU VRAM
    let gpu_label = if hw.gpu_name.is_empty() { "Dedicated GPU".to_string() } else { hw.gpu_name.clone() };
    let vram_check = if hw.gpu_vram_mb >= 12_000 {
        Check {
            id: "gpu_vram",
            title: "GPU VRAM".into(),
            status: Status::Ready,
            detail: format!("{gpu_label} ({:.0} GB) — ample for high textures & ray tracing", hw.gpu_vram_mb as f64 / 1024.0),
            action_tweak: None,
        }
    } else if hw.gpu_vram_mb >= 8_000 {
        Check {
            id: "gpu_vram",
            title: "GPU VRAM".into(),
            status: Status::Info,
            detail: format!("{gpu_label} ({:.0} GB) — good for 1080p/1440p; watch Ray Tracing VRAM", hw.gpu_vram_mb as f64 / 1024.0),
            action_tweak: None,
        }
    } else if hw.gpu_vram_mb > 0 {
        Check {
            id: "gpu_vram",
            title: "GPU VRAM".into(),
            status: Status::Warn,
            detail: format!("{gpu_label} ({:.1} GB) — tight for ray-traced open worlds", hw.gpu_vram_mb as f64 / 1024.0),
            action_tweak: None,
        }
    } else {
        Check {
            id: "gpu_vram",
            title: "GPU VRAM".into(),
            status: Status::Info,
            detail: "Dedicated GPU VRAM not reported".into(),
            action_tweak: None,
        }
    };
    checks.push(vram_check);

    // 2. System RAM
    let ram_gb = hw.total_ram_mb as f64 / 1024.0;
    let ram_check = if hw.total_ram_mb >= 30_000 {
        Check {
            id: "system_ram",
            title: "System RAM".into(),
            status: Status::Ready,
            detail: format!("{ram_gb:.0} GB — plenty of headroom for dense open worlds"),
            action_tweak: None,
        }
    } else if hw.total_ram_mb >= 15_000 {
        Check {
            id: "system_ram",
            title: "System RAM".into(),
            status: Status::Info,
            detail: format!("{ram_gb:.0} GB — close browsers & keep Standby purge on"),
            action_tweak: None,
        }
    } else if hw.total_ram_mb > 0 {
        Check {
            id: "system_ram",
            title: "System RAM".into(),
            status: Status::Warn,
            detail: format!("{ram_gb:.0} GB — below modern open-world baseline"),
            action_tweak: None,
        }
    } else {
        Check {
            id: "system_ram",
            title: "System RAM".into(),
            status: Status::Info,
            detail: "System RAM total unavailable".into(),
            action_tweak: None,
        }
    };
    checks.push(ram_check);

    // 3. System drive SSD/NVMe check
    if let Some(ssd) = hw.system_drive_ssd {
        let other_drive_open_world = installed.iter().any(|g| {
            let n = g.name.to_ascii_lowercase();
            let is_rockstar = n.contains("grand theft auto") || n.contains("red dead");
            let d = g.dir.to_string_lossy();
            is_rockstar && d.len() >= 2 && !d.to_ascii_uppercase().starts_with("C:")
        });
        let detail = match (ssd, other_drive_open_world) {
            (true, true) => "System drive is SSD — check your game drive is an SSD too".to_string(),
            (true, false) => "SSD detected — fast asset streaming".to_string(),
            (false, _) => "Mechanical HDD detected — open-world streaming will stutter".to_string(),
        };
        checks.push(Check {
            id: "storage_ssd",
            title: "Storage (SSD)".into(),
            status: if ssd { Status::Ready } else { Status::Warn },
            detail,
            action_tweak: None,
        });
    }

    // 4. HAGS (needed for DLSS / FSR Frame Generation)
    let hags = match state_of("hags_on") {
        TweakState::Applied | TweakState::AlreadySet => Check {
            id: "hags",
            title: "HAGS (Frame Gen)".into(),
            status: Status::Ready,
            detail: "On — ready for DLSS/FSR Frame Generation".into(),
            action_tweak: None,
        },
        TweakState::Unavailable(reason) => Check {
            id: "hags",
            title: "HAGS (Frame Gen)".into(),
            status: Status::Info,
            detail: reason,
            action_tweak: None,
        },
        _ => Check {
            id: "hags",
            title: "HAGS (Frame Gen)".into(),
            status: Status::Info,
            detail: "Needed for DLSS/FSR Frame Generation; restart required".into(),
            action_tweak: Some("hags_on"),
        },
    };
    checks.push(hags);

    // 5. Core Windows gaming basics
    let basics_ok = ["game_mode_on", "game_dvr_off", "windowed_flip_model"]
        .into_iter()
        .all(|id| matches!(state_of(id), TweakState::Applied | TweakState::AlreadySet | TweakState::Unavailable(_)));
    checks.push(Check {
        id: "windows_basics",
        title: "Windows basics".into(),
        status: if basics_ok { Status::Ready } else { Status::Warn },
        detail: if basics_ok {
            "Game Mode on, background capture off, flip model ready".into()
        } else {
            "Enable LeanFPS Recommended preset for core Windows fixes".into()
        },
        action_tweak: None,
    });

    // 6. Read-only Memory Integrity (VBS) notice — excluded from readiness denominator.
    if hw.memory_integrity_on == Some(true) {
        checks.push(Check {
            id: "memory_integrity",
            title: "Memory Integrity (VBS)".into(),
            status: Status::Info,
            detail: "On — protects Windows security. LeanFPS never disables security features.".into(),
            action_tweak: None,
        });
    }

    let total_checks = checks.iter().filter(|c| c.id != "memory_integrity").count();
    let ready_count = checks.iter().filter(|c| c.id != "memory_integrity" && c.status == Status::Ready).count();

    ReadinessReport { checks, ready_count, total_checks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, RegData};
    use crate::games::library::Store as LibStore;
    use crate::platform::mock::MockPlatform;
    use crate::store::Store;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_engine() -> (Engine, Arc<MockPlatform>) {
        let pf = Arc::new(MockPlatform::new());
        let e = Engine::new(Catalog::builtin(), pf.clone(), Store::temp());
        (e, pf)
    }

    #[test]
    fn high_end_pc_with_tweaks_is_fully_ready() {
        let (mut e, pf) = test_engine();
        pf.with(|s| {
            s.reg.insert(("HKLM\\SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(), "HwSchMode".into()), RegData::Dword(2));
        });
        e.apply("game_mode_on").unwrap();
        e.apply("game_dvr_off").unwrap();
        e.apply("windowed_flip_model").unwrap();
        let hw = HardwareInfo {
            gpu_name: "RTX 4080".into(),
            gpu_vram_mb: 16384,
            total_ram_mb: 32768,
            system_drive_ssd: Some(true),
            memory_integrity_on: Some(true),
        };
        let r = evaluate(&hw, &e, &[]);
        assert_eq!((r.ready_count, r.total_checks), (5, 5));
        assert!(r.checks.iter().any(|c| c.id == "memory_integrity" && c.action_tweak.is_none()));
    }

    #[test]
    fn mid_range_pc_shows_actionable_hags_and_16gb_ram_info() {
        let (e, _) = test_engine();
        let hw = HardwareInfo {
            gpu_name: "RTX 4060".into(),
            gpu_vram_mb: 8192,
            total_ram_mb: 16384,
            system_drive_ssd: Some(true),
            memory_integrity_on: None,
        };
        let installed = vec![Installed {
            name: "Grand Theft Auto V Enhanced".into(),
            dir: PathBuf::from(r"D:\Games\GTAV"),
            store: LibStore::Rockstar,
            steam_appid: None,
            exe: None,
        }];
        let r = evaluate(&hw, &e, &installed);
        assert_eq!(r.checks.iter().find(|c| c.id == "gpu_vram").unwrap().status, Status::Info);
        assert_eq!(r.checks.iter().find(|c| c.id == "system_ram").unwrap().status, Status::Info);
        assert!(r.checks.iter().find(|c| c.id == "storage_ssd").unwrap().detail.contains("game drive"));
        let hags = r.checks.iter().find(|c| c.id == "hags").unwrap();
        assert_eq!(hags.action_tweak, Some("hags_on"));
        assert!(!r.checks.iter().any(|c| c.id == "memory_integrity"));
    }

    #[test]
    fn hdd_low_vram_and_low_ram_warn() {
        let (e, _) = test_engine();
        let hw = HardwareInfo {
            gpu_name: "GTX 1650".into(),
            gpu_vram_mb: 4096,
            total_ram_mb: 8192,
            system_drive_ssd: Some(false),
            memory_integrity_on: Some(false),
        };
        let r = evaluate(&hw, &e, &[]);
        assert_eq!(r.checks.iter().find(|c| c.id == "gpu_vram").unwrap().status, Status::Warn);
        assert_eq!(r.checks.iter().find(|c| c.id == "system_ram").unwrap().status, Status::Warn);
        assert_eq!(r.checks.iter().find(|c| c.id == "storage_ssd").unwrap().status, Status::Warn);
        assert_eq!(r.checks.iter().find(|c| c.id == "windows_basics").unwrap().status, Status::Warn);
        assert_eq!(r.ready_count, 0);
    }
}
