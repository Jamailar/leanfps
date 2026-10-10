//! Tweak catalog: what LeanFPS can change, loaded from `data/tweaks.toml`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Written once, stays until the user reverts it.
    Persistent,
    /// Applied when a game starts, reverted when it exits.
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Low => "Low risk",
            Risk::Medium => "Medium risk",
            Risk::High => "High risk",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Genre {
    Shooter,
    Moba,
    Strategy,
    OpenWorld,
    Racing,
    Sandbox,
    Mmo,
    Casual,
}

impl Genre {
    pub const ALL: [Genre; 8] = [
        Genre::Shooter,
        Genre::Moba,
        Genre::Strategy,
        Genre::OpenWorld,
        Genre::Racing,
        Genre::Sandbox,
        Genre::Mmo,
        Genre::Casual,
    ];

    /// The snake_case id used in data files.
    pub fn from_code(code: &str) -> Option<Genre> {
        Genre::ALL.into_iter().find(|g| g.code() == code)
    }

    pub fn code(self) -> &'static str {
        match self {
            Genre::Shooter => "shooter",
            Genre::Moba => "moba",
            Genre::Strategy => "strategy",
            Genre::OpenWorld => "open_world",
            Genre::Racing => "racing",
            Genre::Sandbox => "sandbox",
            Genre::Mmo => "mmo",
            Genre::Casual => "casual",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Genre::Shooter => "Competitive shooter",
            Genre::Moba => "MOBA",
            Genre::Strategy => "Strategy",
            Genre::OpenWorld => "Open world & RPG",
            Genre::Racing => "Racing & sports",
            Genre::Sandbox => "Sandbox & survival",
            Genre::Mmo => "Online RPG",
            Genre::Casual => "Casual & indie",
        }
    }
}

/// CPU layout a tweak needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuNeed {
    #[default]
    Any,
    /// Two L3 caches of different size (AMD X3D with one V-Cache die).
    SplitCache,
    /// Performance and efficiency cores.
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form {
    #[default]
    Any,
    Desktop,
    Laptop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::Idle => "Idle",
            Priority::BelowNormal => "Below normal",
            Priority::Normal => "Normal",
            Priority::AboveNormal => "Above normal",
            Priority::High => "High",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegData {
    Dword(u32),
    Sz(String),
    /// Any other registry type (QWORD, binary, expandable or multi string), kept byte for
    /// byte so restoring it puts back exactly what was there.
    Raw { ty: u32, data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Set a registry value. `key` starts with `HKCU\` or `HKLM\`.
    Registry { key: String, name: String, value: RegData },
    /// Make a power scheme the active one.
    PowerScheme { scheme: String },
    /// Change a setting of the active power scheme. `dc` = None leaves battery value alone.
    PowerSetting {
        subgroup: String,
        setting: String,
        ac: u32,
        #[serde(default)]
        dc: Option<u32>,
    },
    /// "Enhance pointer precision". Applied live, no sign-out needed.
    MouseAcceleration { enabled: bool },
    /// CPU priority class of the running game.
    GamePriority { class: Priority },
    /// Opt the running game out of EcoQoS power throttling.
    GamePowerThrottlingOff,
    /// Keep the PC and display awake while the game runs.
    KeepAwake,
    /// Lower the priority of known background apps while the game runs.
    BackgroundPriority { class: Priority, processes: Vec<String> },
    /// Purge the standby memory list when free memory drops below a threshold.
    StandbyPurge { below_free_mb: u64 },
    /// Windows power mode overlay ("Best performance") while the game runs.
    PowerOverlay { overlay: String },
    /// Per-game graphics preference (2 = high performance GPU). Kept after the game exits,
    /// because Windows reads it when the game starts; undone with Restore all.
    GpuPreference { value: u32 },
    /// Run the game on a subset of cores (Windows CPU sets) while it runs. Skipped when the
    /// game already has its own core choice.
    GameCpuSets { prefer: crate::platform::CpuPrefer },
    /// Set one `field=value;` entry inside a registry string such as
    /// `DirectXUserGlobalSettings`, keeping the other entries. Undo restores only that entry.
    RegistryField { key: String, name: String, field: String, value: String },
}

impl Action {
    /// Needs a running game.
    pub fn is_session_only(&self) -> bool {
        matches!(
            self,
            Action::GamePriority { .. }
                | Action::GamePowerThrottlingOff
                | Action::KeepAwake
                | Action::BackgroundPriority { .. }
                | Action::StandbyPurge { .. }
                | Action::PowerOverlay { .. }
                | Action::GpuPreference { .. }
                | Action::GameCpuSets { .. }
        )
    }

    /// Stays in place per game after the session ends.
    pub fn is_sticky(&self) -> bool {
        matches!(self, Action::GpuPreference { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tweak {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub detail: String,
    pub scope: Scope,
    pub risk: Risk,
    #[serde(default)]
    pub reboot: bool,
    /// Presets that turn this tweak on.
    #[serde(default)]
    pub presets: Vec<String>,
    #[serde(default)]
    pub pro: bool,
    /// Session tweaks only: limit to these genres. Empty = every game.
    #[serde(default)]
    pub genres: Vec<Genre>,
    #[serde(default)]
    pub form: Form,
    #[serde(default)]
    pub min_build: u32,
    #[serde(default)]
    pub cpu: CpuNeed,
    #[serde(default)]
    pub sources: Vec<String>,
    pub actions: Vec<Action>,
}

impl Tweak {
    pub fn is_genre_specific(&self) -> bool {
        !self.genres.is_empty()
    }

    pub fn applies_to_genre(&self, genre: Option<Genre>) -> bool {
        self.genres.is_empty() || genre.is_some_and(|g| self.genres.contains(&g))
    }

    /// Suits this PC (desktop/laptop, Windows build).
    pub fn fits(&self, m: &crate::platform::Machine) -> bool {
        let form_ok = match self.form {
            Form::Any => true,
            Form::Desktop => !m.laptop,
            Form::Laptop => m.laptop,
        };
        let cpu_ok = match self.cpu {
            CpuNeed::Any => true,
            CpuNeed::SplitCache => m.split_cache,
            CpuNeed::Hybrid => m.hybrid,
        };
        form_ok && cpu_ok && (self.min_build == 0 || m.os_build == 0 || m.os_build >= self.min_build)
    }

    pub fn in_preset(&self, preset: &str) -> bool {
        self.presets.iter().any(|p| p == preset)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PresetDef {
    pub id: String,
    pub title: String,
    pub summary: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub preset: Vec<PresetDef>,
    pub tweak: Vec<Tweak>,
}

/// Processes whose priority LeanFPS must never lower: slowing them can freeze input, sound
/// or the desktop.
pub const PROTECTED_PROCESSES: [&str; 14] = [
    "system", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "svchost.exe", "dwm.exe",
    "explorer.exe", "audiodg.exe", "fontdrvhost.exe", "ctfmon.exe", "leanfps.exe",
];

pub const BUILTIN_TWEAKS: &str = include_str!("../data/tweaks.toml");

impl Catalog {
    pub fn builtin() -> Catalog {
        Catalog::parse(BUILTIN_TWEAKS).expect("built-in tweak catalog is valid")
    }

    pub fn parse(src: &str) -> Result<Catalog, String> {
        let cat: Catalog = toml::from_str(src).map_err(|e| e.to_string())?;
        cat.validate()?;
        Ok(cat)
    }

    fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for t in &self.tweak {
            if !seen.insert(t.id.as_str()) {
                return Err(format!("duplicate tweak id {}", t.id));
            }
            if t.actions.is_empty() {
                return Err(format!("tweak {} has no actions", t.id));
            }
            for p in &t.presets {
                if !self.preset.iter().any(|d| &d.id == p) {
                    return Err(format!("tweak {} uses unknown preset {p}", t.id));
                }
            }
            if t.scope == Scope::Persistent && !t.genres.is_empty() {
                return Err(format!("persistent tweak {} cannot be genre-specific", t.id));
            }
            for a in &t.actions {
                if a.is_session_only() && t.scope != Scope::Session {
                    return Err(format!("tweak {}: action {a:?} needs session scope", t.id));
                }
                if let Action::BackgroundPriority { processes, .. } = a {
                    if let Some(p) = processes.iter().find(|p| PROTECTED_PROCESSES.contains(&p.to_ascii_lowercase().as_str())) {
                        return Err(format!("tweak {}: {p} is a Windows process and must never be slowed down", t.id));
                    }
                }
                if let Action::Registry { key, name, .. } | Action::RegistryField { key, name, .. } = a {
                    let k = key.to_ascii_lowercase();
                    if k.contains("image file execution options") || name.eq_ignore_ascii_case("Debugger") {
                        return Err(format!("tweak {}: Image File Execution Options is only for launch priority, set in code", t.id));
                    }
                }
                if let Action::Registry { key, .. } | Action::RegistryField { key, .. } = a {
                    if !(key.starts_with("HKCU\\") || key.starts_with("HKLM\\")) {
                        return Err(format!("tweak {}: registry key must start with HKCU\\ or HKLM\\", t.id));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Tweak> {
        self.tweak.iter().find(|t| t.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_parses() {
        let c = Catalog::builtin();
        assert!(c.tweak.len() >= 10);
        assert!(c.preset.iter().any(|p| p.id == "recommended"));
        // Every genre has at least one genre-specific tweak or relies on the generic set.
        assert!(c.tweak.iter().any(|t| t.genres.contains(&Genre::Shooter)));
    }

    #[test]
    fn rejects_persistent_genre_tweak() {
        let src = r#"
[[preset]]
id = "recommended"
title = "R"
summary = "R"
[[tweak]]
id = "x"
title = "x"
summary = "x"
detail = "x"
scope = "persistent"
risk = "low"
genres = ["shooter"]
actions = [{ type = "keep_awake" }]
"#;
        assert!(Catalog::parse(src).is_err());
    }

    #[test]
    fn rejects_lowering_windows_processes() {
        let src = r#"
[[preset]]
id = "recommended"
title = "R"
summary = "R"
[[tweak]]
id = "x"
title = "x"
summary = "x"
detail = "x"
scope = "session"
risk = "low"
actions = [{ type = "background_priority", class = "below_normal", processes = ["chrome.exe", "Svchost.exe"] }]
"#;
        assert!(Catalog::parse(src).is_err());
    }
}
