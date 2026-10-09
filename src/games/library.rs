//! Installed games from Steam and Epic, read from their local files. No network.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Steam,
    Epic,
    Gog,
    BattleNet,
    Ubisoft,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub dir: PathBuf,
    pub store: Store,
    pub steam_appid: Option<u32>,
    /// Main executable when the store tells us (Epic does, Steam doesn't).
    pub exe: Option<PathBuf>,
}

// ------------------------------------------------------------------ VDF (Valve KeyValues)

#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Str(String),
    Map(Vec<(String, Vdf)>),
}

impl Vdf {
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Map(items) => items.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Vdf::Str(s) => Some(s),
            Vdf::Map(_) => None,
        }
    }
    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Map(items) => items,
            Vdf::Str(_) => &[],
        }
    }
}

/// Parses the text KeyValues format used by `libraryfolders.vdf` and `appmanifest_*.acf`.
pub fn parse_vdf(src: &str) -> Option<Vdf> {
    let mut tokens = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(o) => s.push(o),
                            None => break,
                        },
                        '"' => break,
                        o => s.push(o),
                    }
                }
                tokens.push(Some(s));
            }
            '{' => tokens.push(None),
            '}' => tokens.push(Some("\u{0}}".into())),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    fn map(tokens: &[Option<String>], i: &mut usize) -> Option<Vec<(String, Vdf)>> {
        let mut items = Vec::new();
        while *i < tokens.len() {
            let key = tokens[*i].clone()?;
            *i += 1;
            if key == "\u{0}}" {
                return Some(items);
            }
            match tokens.get(*i)? {
                None => {
                    *i += 1;
                    items.push((key, Vdf::Map(map(tokens, i)?)));
                }
                Some(v) => {
                    *i += 1;
                    items.push((key, Vdf::Str(v.clone())));
                }
            }
        }
        Some(items)
    }
    let mut i = 0;
    Some(Vdf::Map(map(&tokens, &mut i)?))
}

// ------------------------------------------------------------------ Steam

pub fn steam_library_paths(steam_root: &Path) -> Vec<PathBuf> {
    let mut out = vec![steam_root.to_path_buf()];
    let file = steam_root.join("steamapps").join("libraryfolders.vdf");
    if let Some(v) = std::fs::read_to_string(&file).ok().and_then(|s| parse_vdf(&s)) {
        if let Some(lf) = v.get("libraryfolders") {
            for (_, entry) in lf.entries() {
                // Newer format: { "path" "..." }, older: "1" "D:\\Games"
                let p = match entry {
                    Vdf::Str(s) => Some(s.as_str()),
                    Vdf::Map(_) => entry.str("path"),
                };
                if let Some(p) = p {
                    let pb = PathBuf::from(p);
                    if !out.iter().any(|o| same_path(o, &pb)) {
                        out.push(pb);
                    }
                }
            }
        }
    }
    out
}

pub fn steam_games(steam_root: &Path) -> Vec<Installed> {
    let mut games = Vec::new();
    for lib in steam_library_paths(steam_root) {
        let apps = lib.join("steamapps");
        let Ok(rd) = std::fs::read_dir(&apps) else { continue };
        for f in rd.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let Some(v) = std::fs::read_to_string(f.path()).ok().and_then(|s| parse_vdf(&s)) else { continue };
            let Some(st) = v.get("AppState") else { continue };
            let (Some(appid), Some(title), Some(dir)) = (st.str("appid"), st.str("name"), st.str("installdir")) else { continue };
            let Ok(appid) = appid.parse::<u32>() else { continue };
            // Tools and runtimes are not games.
            if title.starts_with("Steamworks") || title.starts_with("Proton") || title.contains("Redistributable") || appid == 228980 {
                continue;
            }
            games.push(Installed {
                name: title.to_string(),
                dir: apps.join("common").join(dir),
                store: Store::Steam,
                steam_appid: Some(appid),
                exe: None,
            });
        }
    }
    games
}

// ------------------------------------------------------------------ Epic

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct EpicItem {
    #[serde(default)]
    DisplayName: Option<String>,
    #[serde(default)]
    AppName: Option<String>,
    #[serde(default)]
    InstallLocation: Option<String>,
    #[serde(default)]
    LaunchExecutable: Option<String>,
    #[serde(default)]
    bIsApplication: Option<bool>,
}

pub fn epic_games(manifest_dir: &Path) -> Vec<Installed> {
    let mut games = Vec::new();
    let Ok(rd) = std::fs::read_dir(manifest_dir) else { return games };
    for f in rd.flatten() {
        if f.path().extension().and_then(|e| e.to_str()) != Some("item") {
            continue;
        }
        // Manifests can be empty or broken; skip those.
        let Some(item) = std::fs::read(f.path()).ok().and_then(|b| serde_json::from_slice::<EpicItem>(&b).ok()) else { continue };
        if item.bIsApplication == Some(false) {
            continue;
        }
        let Some(loc) = item.InstallLocation.filter(|l| !l.is_empty()) else { continue };
        let name = item.DisplayName.or(item.AppName).unwrap_or_else(|| "Epic game".into());
        let dir = PathBuf::from(&loc);
        let exe = item.LaunchExecutable.filter(|e| !e.is_empty()).map(|e| dir.join(e));
        games.push(Installed { name, dir, store: Store::Epic, steam_appid: None, exe });
    }
    games
}

// ------------------------------------------------------------------ installed-programs list
//
// Patterns follow Playnite's library plugins (MIT): GOG games register as `<id>_is1` with
// publisher "GOG.com"; Battle.net games have an uninstall string with `Battle.net` and `--uid=`;
// Steam adds `Steam App <appid>` entries.

use crate::platform::UninstallEntry;

pub fn from_uninstall(entries: &[UninstallEntry]) -> Vec<Installed> {
    let mut out = Vec::new();
    for e in entries {
        if e.install_location.trim().is_empty() || e.display_name.trim().is_empty() {
            continue;
        }
        let key = e.key.as_str();
        let store = if e.publisher == "GOG.com" && key.ends_with("_is1") && key[..key.len() - 4].chars().all(|c| c.is_ascii_digit()) {
            Some((Store::Gog, None))
        } else if e.uninstall_string.contains("Battle.net") && e.uninstall_string.contains("--uid=") {
            Some((Store::BattleNet, None))
        } else if let Some(id) = key.strip_prefix("Steam App ").and_then(|id| id.parse::<u32>().ok()) {
            Some((Store::Steam, Some(id)))
        } else {
            None
        };
        if let Some((store, steam_appid)) = store {
            let dir = e.install_location.trim().trim_matches('"').replace('/', "\\");
            out.push(Installed { name: e.display_name.trim().to_string(), dir: PathBuf::from(dir), store, steam_appid, exe: None });
        }
    }
    out
}

/// Ubisoft Connect installs: (folder name from registry, install dir). The launcher doesn't
/// store a title there, so the folder name is used.
pub fn from_ubisoft(installs: &[String]) -> Vec<Installed> {
    installs
        .iter()
        .filter(|d| !d.trim().is_empty())
        .map(|d| {
            let dir = d.trim().replace('/', "\\");
            let dir = dir.trim_end_matches('\\').to_string();
            let name = dir.rsplit('\\').next().unwrap_or(&dir).to_string();
            Installed { name, dir: PathBuf::from(dir), store: Store::Ubisoft, steam_appid: None, exe: None }
        })
        .collect()
}

// ------------------------------------------------------------------ folder conventions

/// Install folders that only hold games: (marker, launcher). The folder right after the
/// marker is the game's own folder and its name is used as the game name.
const GAME_ROOTS: &[&str] = &[
    "\\steamapps\\common\\",
    "\\epic games\\",
    "\\gog galaxy\\games\\",
    "\\gog games\\",
    "\\xboxgames\\",
    "\\riot games\\",
    "\\ubisoft game launcher\\games\\",
    "\\ea games\\",
    "\\rockstar games\\",
    "\\amazon games\\library\\",
    "\\itch\\apps\\",
    "\\battlestate games\\",
];

/// If `exe_path` sits in a well-known game folder, the game's folder name.
pub fn game_folder(exe_path: &str) -> Option<String> {
    let p = normalize(exe_path);
    for marker in GAME_ROOTS {
        if let Some(i) = p.find(marker) {
            let rest = &exe_path.replace('/', "\\")[i + marker.len()..];
            let folder = rest.split('\\').next().unwrap_or("");
            // The exe itself is not a folder (e.g. a launcher sitting in the root).
            if !folder.is_empty() && rest.contains('\\') {
                return Some(folder.to_string());
            }
        }
    }
    None
}

// ------------------------------------------------------------------ helpers

pub fn normalize(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

fn same_path(a: &Path, b: &Path) -> bool {
    normalize(&a.to_string_lossy()) == normalize(&b.to_string_lossy())
}

/// The installed game whose folder contains `exe_path`, longest match first.
pub fn owner_of<'a>(games: &'a [Installed], exe_path: &str) -> Option<&'a Installed> {
    let exe = normalize(exe_path);
    games
        .iter()
        .filter(|g| {
            let d = normalize(&g.dir.to_string_lossy());
            exe.starts_with(&d) && exe.as_bytes().get(d.len()) == Some(&b'\\')
        })
        .max_by_key(|g| g.dir.as_os_str().len())
}

/// Scan everything we know about. Paths come from the platform layer on Windows.
pub fn scan(steam_root: Option<&Path>, epic_manifests: Option<&Path>, uninstall: &[UninstallEntry], ubisoft: &[String]) -> Vec<Installed> {
    let mut all = Vec::new();
    if let Some(r) = steam_root {
        all.extend(steam_games(r));
    }
    if let Some(e) = epic_manifests {
        all.extend(epic_games(e));
    }
    all.extend(from_uninstall(uninstall));
    all.extend(from_ubisoft(ubisoft));
    let mut seen = HashMap::new();
    all.retain(|g| seen.insert(normalize(&g.dir.to_string_lossy()), ()).is_none());
    all.sort_by_key(|g| g.name.to_lowercase());
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_programs_and_ubisoft() {
        let e = |key: &str, name: &str, publisher: &str, loc: &str, un: &str| UninstallEntry {
            key: key.into(),
            display_name: name.into(),
            publisher: publisher.into(),
            install_location: loc.into(),
            uninstall_string: un.into(),
        };
        let entries = vec![
            e("1207658924_is1", "The Witcher 3", "GOG.com", r"C:\GOG Games\The Witcher 3", ""),
            e("GOGPACK_x_is1", "Pack", "GOG.com", r"C:\x", ""),
            e("Overwatch", "Overwatch", "Blizzard Entertainment", r"C:\Program Files (x86)\Overwatch", r#""C:\Battle.net\Battle.net.exe" --uid=prometheus --lang=enUS"#),
            e("Steam App 570", "Dota 2", "Valve", r"D:\Steam\steamapps\common\dota 2 beta", ""),
            e("{GUID}", "7-Zip", "Igor Pavlov", r"C:\Program Files\7-Zip", ""),
            e("Steam App 1", "No location", "Valve", "", ""),
        ];
        let got = from_uninstall(&entries);
        let names: Vec<_> = got.iter().map(|g| (g.name.as_str(), g.store, g.steam_appid)).collect();
        assert_eq!(names, vec![("The Witcher 3", Store::Gog, None), ("Overwatch", Store::BattleNet, None), ("Dota 2", Store::Steam, Some(570))]);
        let ubi = from_ubisoft(&["C:/Program Files (x86)/Ubisoft/Ubisoft Game Launcher/games/Tom Clancy's Rainbow Six Siege/".into()]);
        assert_eq!(ubi[0].name, "Tom Clancy's Rainbow Six Siege");
        assert!(owner_of(&ubi, r"C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\games\Tom Clancy's Rainbow Six Siege\RainbowSix.exe").is_some());
    }

    #[test]
    fn game_folders() {
        assert_eq!(game_folder(r"C:\XboxGames\Forza Horizon 5\Content\ForzaHorizon5.exe").as_deref(), Some("Forza Horizon 5"));
        assert_eq!(game_folder(r"C:\Riot Games\VALORANT\live\VALORANT.exe").as_deref(), Some("VALORANT"));
        assert_eq!(game_folder(r"D:\Epic Games\Fortnite\FortniteGame\Binaries\Win64\x.exe").as_deref(), Some("Fortnite"));
        assert_eq!(game_folder(r"C:\Riot Games\RiotClient.exe"), None);
        assert_eq!(game_folder(r"C:\Program Files\Notepad++\notepad++.exe"), None);
    }

    #[test]
    fn parses_libraryfolders() {
        let src = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps" { "730" "123" }
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}"#;
        let v = parse_vdf(src).unwrap();
        let lf = v.get("libraryfolders").unwrap();
        assert_eq!(lf.entries().len(), 2);
        assert_eq!(lf.get("1").unwrap().str("path"), Some("D:\\SteamLibrary"));
        assert_eq!(lf.get("0").unwrap().get("apps").unwrap().str("730"), Some("123"));
    }

    #[test]
    fn scans_steam_and_epic_dirs() {
        let root = std::env::temp_dir().join(format!("leanfps-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let steam = root.join("Steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(
            steam.join("steamapps").join("appmanifest_730.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"name\"\t\t\"Counter-Strike 2\"\n\t\"installdir\"\t\t\"Counter-Strike Global Offensive\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            steam.join("steamapps").join("appmanifest_228980.acf"),
            "\"AppState\" { \"appid\" \"228980\" \"name\" \"Steamworks Common Redistributables\" \"installdir\" \"x\" }",
        )
        .unwrap();
        let epic = root.join("Manifests");
        std::fs::create_dir_all(&epic).unwrap();
        std::fs::write(
            epic.join("a.item"),
            r#"{"DisplayName":"Fortnite","AppName":"Fortnite","InstallLocation":"C:\\Program Files\\Epic Games\\Fortnite","LaunchExecutable":"FortniteGame/Binaries/Win64/FortniteLauncher.exe","bIsApplication":true}"#,
        )
        .unwrap();
        std::fs::write(epic.join("broken.item"), "").unwrap();

        let games = scan(Some(&steam), Some(&epic), &[], &[]);
        let names: Vec<_> = games.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["Counter-Strike 2", "Fortnite"]);
        let cs = &games[0];
        let exe = format!("{}\\game\\bin\\win64\\cs2.exe", cs.dir.to_string_lossy());
        assert_eq!(owner_of(&games, &exe).unwrap().steam_appid, Some(730));
        assert!(owner_of(&games, "C:\\Other\\cs2.exe").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
