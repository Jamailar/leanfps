//! Which game is running, what it's called and what kind of game it is.

pub mod library;

use crate::catalog::Genre;
use crate::platform::file_name_lower;
use library::Installed;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct KnownGame {
    pub name: String,
    pub genre: Genre,
    pub exe: Vec<String>,
    #[serde(default)]
    pub steam: Option<u32>,
    /// All of these must appear (case-insensitive) in the full path, for generic exes like javaw.exe.
    #[serde(default)]
    pub path_contains: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GenreInfo {
    pub id: Genre,
    #[serde(default)]
    pub tips: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameDb {
    pub genre: Vec<GenreInfo>,
    pub game: Vec<KnownGame>,
}

pub const BUILTIN_GAMES: &str = include_str!("../../data/games.toml");

impl GameDb {
    pub fn builtin() -> GameDb {
        toml::from_str(BUILTIN_GAMES).expect("built-in game database is valid")
    }

    pub fn tips(&self, g: Genre) -> &[String] {
        self.genre.iter().find(|x| x.id == g).map(|x| x.tips.as_slice()).unwrap_or(&[])
    }

    pub fn by_exe(&self, exe_path: &str) -> Option<&KnownGame> {
        let file = file_name_lower(exe_path);
        let path = exe_path.to_ascii_lowercase();
        self.game.iter().find(|g| {
            g.exe.iter().any(|e| e.eq_ignore_ascii_case(&file))
                && g.path_contains.iter().all(|p| path.contains(&p.to_ascii_lowercase()))
        })
    }

    pub fn by_steam(&self, appid: u32) -> Option<&KnownGame> {
        self.game.iter().find(|g| g.steam == Some(appid))
    }

    pub fn by_name(&self, name: &str) -> Option<&KnownGame> {
        self.game.iter().find(|g| g.name.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Matched our built-in database.
    Known,
    /// Lives in a Steam or Epic game folder.
    Library,
    /// Unknown app running fullscreen.
    Fullscreen,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GameInfo {
    pub name: String,
    pub genre: Option<Genre>,
    pub exe_path: String,
    pub source: Source,
}

/// Apps that run fullscreen but are not games.
const NOT_GAMES: &[&str] = &[
    "explorer.exe", "applicationframehost.exe", "searchhost.exe", "startmenuexperiencehost.exe",
    "shellexperiencehost.exe", "lockapp.exe", "chrome.exe", "msedge.exe", "firefox.exe", "opera.exe",
    "brave.exe", "vivaldi.exe", "vlc.exe", "mpc-hc64.exe", "mpc-be64.exe", "potplayermini64.exe",
    "powerpnt.exe", "obs64.exe", "steam.exe", "steamwebhelper.exe", "epicgameslauncher.exe",
    "leanfps.exe", "taskmgr.exe", "mstsc.exe", "vmware.exe", "virtualboxvm.exe", "zoom.exe",
    "ms-teams.exe", "discord.exe", "spotify.exe", "code.exe", "devenv.exe",
];

/// Decide whether `exe_path` is a game. `user_genres` are the genres the user picked for
/// unknown games, keyed by lower-case exe file name.
pub fn classify(
    exe_path: &str,
    fullscreen: bool,
    db: &GameDb,
    installed: &[Installed],
    user_genres: &HashMap<String, Genre>,
) -> Option<GameInfo> {
    let file = file_name_lower(exe_path);
    let user = user_genres.get(&file).copied();
    if let Some(k) = db.by_exe(exe_path) {
        return Some(GameInfo { name: k.name.clone(), genre: Some(user.unwrap_or(k.genre)), exe_path: exe_path.into(), source: Source::Known });
    }
    if NOT_GAMES.contains(&file.as_str()) {
        return None;
    }
    if let Some(owner) = library::owner_of(installed, exe_path) {
        let known = owner.steam_appid.and_then(|id| db.by_steam(id)).or_else(|| db.by_name(&owner.name));
        return Some(GameInfo {
            name: known.map(|k| k.name.clone()).unwrap_or_else(|| owner.name.clone()),
            genre: user.or(known.map(|k| k.genre)),
            exe_path: exe_path.into(),
            source: Source::Library,
        });
    }
    if fullscreen || user.is_some() {
        let stem = file.trim_end_matches(".exe");
        return Some(GameInfo { name: pretty_name(stem), genre: user, exe_path: exe_path.into(), source: Source::Fullscreen });
    }
    None
}

fn pretty_name(stem: &str) -> String {
    let base = stem.split(['-', '_']).next().unwrap_or(stem);
    let mut c = base.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => "Unknown game".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn db_parses_and_has_unique_exes() {
        let db = GameDb::builtin();
        assert!(db.game.len() >= 50);
        let mut seen = std::collections::HashSet::new();
        for g in &db.game {
            for e in &g.exe {
                assert!(seen.insert((e.to_ascii_lowercase(), g.path_contains.clone())), "duplicate exe {e}");
            }
        }
        for g in Genre::ALL {
            assert!(!db.tips(g).is_empty(), "no tips for {g:?}");
        }
    }

    #[test]
    fn classifies_known_library_fullscreen_and_non_games() {
        let db = GameDb::builtin();
        let none = HashMap::new();
        let cs = classify(r"D:\SteamLibrary\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe", false, &db, &[], &none).unwrap();
        assert_eq!((cs.name.as_str(), cs.genre, cs.source), ("Counter-Strike 2", Some(Genre::Shooter), Source::Known));

        // Generic Java only counts as Minecraft inside a Minecraft folder.
        assert!(classify(r"C:\Program Files\Java\bin\javaw.exe", false, &db, &[], &none).is_none());
        let mc = classify(r"C:\Users\a\AppData\Roaming\.minecraft\runtime\bin\javaw.exe", false, &db, &[], &none).unwrap();
        assert_eq!(mc.genre, Some(Genre::Sandbox));

        let lib = vec![Installed {
            name: "Some Indie Game".into(),
            dir: PathBuf::from(r"D:\SteamLibrary\steamapps\common\Indie"),
            store: library::Store::Steam,
            steam_appid: Some(999_999),
            exe: None,
        }];
        let indie = classify(r"D:\SteamLibrary\steamapps\common\Indie\bin\indie.exe", false, &db, &lib, &none).unwrap();
        assert_eq!((indie.name.as_str(), indie.genre, indie.source), ("Some Indie Game", None, Source::Library));

        assert!(classify(r"C:\Windows\explorer.exe", true, &db, &lib, &none).is_none());
        assert!(classify(r"C:\Tools\notepad.exe", false, &db, &lib, &none).is_none());
        let fs = classify(r"C:\Games\cool_game-win64.exe", true, &db, &lib, &none).unwrap();
        assert_eq!((fs.name.as_str(), fs.source), ("Cool", Source::Fullscreen));

        let mut user = HashMap::new();
        user.insert("indie.exe".to_string(), Genre::Racing);
        let indie = classify(r"D:\SteamLibrary\steamapps\common\Indie\bin\indie.exe", false, &db, &lib, &user).unwrap();
        assert_eq!(indie.genre, Some(Genre::Racing));
    }
}
