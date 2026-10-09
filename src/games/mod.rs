//! Which game is running, what it's called and what kind of game it is.

pub mod library;

use crate::catalog::Genre;
use crate::platform::file_name_lower;
use library::Installed;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Deserialize)]
pub struct KnownGame {
    pub name: String,
    /// None for things like emulators, where the genre depends on what is being played.
    #[serde(default)]
    pub genre: Option<Genre>,
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
    /// Lives in a launcher library or a well-known game folder.
    Library,
    /// The user said it's a game.
    Confirmed,
    /// Unknown app running fullscreen.
    Fullscreen,
    /// Large window using a 3D API: we ask before treating it as a game.
    Graphics,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GameInfo {
    pub name: String,
    pub genre: Option<Genre>,
    pub exe_path: String,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Game(GameInfo),
    /// Looks like a game; ask the user once.
    Ask(GameInfo),
    NotGame,
}

/// What the user told us, keyed by lower-case exe file name.
#[derive(Debug, Clone, Default)]
pub struct Prefs {
    pub genres: HashMap<String, Genre>,
    pub confirmed: HashSet<String>,
    pub rejected: HashSet<String>,
}

/// Apps that run fullscreen or use 3D but are not games, plus launcher helpers that live in
/// game folders.
const NOT_GAMES: &[&str] = &[
    "explorer.exe", "applicationframehost.exe", "searchhost.exe", "startmenuexperiencehost.exe",
    "shellexperiencehost.exe", "lockapp.exe", "chrome.exe", "msedge.exe", "firefox.exe", "opera.exe",
    "brave.exe", "vivaldi.exe", "vlc.exe", "mpc-hc64.exe", "mpc-be64.exe", "potplayermini64.exe",
    "powerpnt.exe", "obs64.exe", "steam.exe", "steamwebhelper.exe", "epicgameslauncher.exe",
    "leanfps.exe", "taskmgr.exe", "mstsc.exe", "vmware.exe", "virtualboxvm.exe", "zoom.exe",
    "ms-teams.exe", "discord.exe", "spotify.exe", "code.exe", "devenv.exe",
    "epicwebhelper.exe", "unrealcefsubprocess.exe", "crashreportclient.exe", "unitycrashhandler64.exe",
    "riotclientservices.exe", "riotclientux.exe", "riotclientuxrender.exe", "battle.net.exe", "agent.exe",
    "eadesktop.exe", "eabackgroundservice.exe", "upc.exe", "uplaywebcore.exe", "galaxyclient.exe",
    "rockstarlauncher.exe", "socialclubhelper.exe", "xboxpcapp.exe", "gamingservices.exe",
    "blender.exe", "unity.exe", "unrealeditor.exe", "photoshop.exe", "afterfx.exe", "premiere pro.exe",
    "resolve.exe", "nvidia app.exe", "nvcontainer.exe", "radeonsoftware.exe", "msiafterburner.exe",
];

/// Genre for a title from our databases: curated list first, then Steam tags.
fn genre_for(db: &GameDb, tags: &SteamGenres, appid: Option<u32>, name: &str) -> Option<Genre> {
    appid
        .and_then(|id| db.by_steam(id).and_then(|k| k.genre).or_else(|| tags.by_appid(id)))
        .or_else(|| db.by_name(name).and_then(|k| k.genre))
        .or_else(|| tags.by_name(name))
}

/// Decide whether the foreground app is a game.
/// `graphics` is only called when it matters (it inspects the process's modules).
#[allow(clippy::too_many_arguments)]
pub fn classify(
    exe_path: &str,
    fullscreen: bool,
    coverage: f32,
    graphics: &dyn Fn() -> Option<bool>,
    db: &GameDb,
    tags: &SteamGenres,
    installed: &[Installed],
    prefs: &Prefs,
) -> Verdict {
    let file = file_name_lower(exe_path);
    if prefs.rejected.contains(&file) {
        return Verdict::NotGame;
    }
    let user = prefs.genres.get(&file).copied();
    let game = |name: String, genre: Option<Genre>, source| Verdict::Game(GameInfo { name, genre: user.or(genre), exe_path: exe_path.into(), source });

    if let Some(k) = db.by_exe(exe_path) {
        return game(k.name.clone(), k.genre.or_else(|| tags.by_name(&k.name)), Source::Known);
    }
    if NOT_GAMES.contains(&file.as_str()) {
        return Verdict::NotGame;
    }
    if let Some(owner) = library::owner_of(installed, exe_path) {
        let known = owner.steam_appid.and_then(|id| db.by_steam(id)).or_else(|| db.by_name(&owner.name));
        let name = known.map(|k| k.name.clone()).unwrap_or_else(|| owner.name.clone());
        return game(name, genre_for(db, tags, owner.steam_appid, &owner.name), Source::Library);
    }
    if let Some(folder) = library::game_folder(exe_path) {
        let genre = genre_for(db, tags, None, &folder);
        return game(folder, genre, Source::Library);
    }
    let stem = file.trim_end_matches(".exe");
    if prefs.confirmed.contains(&file) || user.is_some() {
        return game(pretty_name(stem), None, Source::Confirmed);
    }
    if fullscreen {
        return game(pretty_name(stem), tags.by_name(stem), Source::Fullscreen);
    }
    let system = exe_path.to_ascii_lowercase().replace('/', "\\").contains(":\\windows\\");
    if coverage >= 0.6 && !system && graphics() == Some(true) {
        return Verdict::Ask(GameInfo { name: pretty_name(stem), genre: None, exe_path: exe_path.into(), source: Source::Graphics });
    }
    Verdict::NotGame
}

// ------------------------------------------------------------------ Steam tag genres

/// Genre per Steam app, built from SteamSpy tag votes (`data/steam_genres.tsv`,
/// generated by `tools/build_genre_db.py`).
#[derive(Debug, Default)]
pub struct SteamGenres {
    appid: HashMap<u32, Genre>,
    name: HashMap<String, Genre>,
}

pub const BUILTIN_STEAM_GENRES: &str = include_str!("../../data/steam_genres.tsv");

/// Lower-case letters and digits only: "Counter-Strike 2" -> "counterstrike2".
pub fn name_key(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

impl SteamGenres {
    pub fn builtin() -> SteamGenres {
        SteamGenres::parse(BUILTIN_STEAM_GENRES)
    }

    /// Lines: `appid<TAB>genre<TAB>name`; `#` starts a comment.
    pub fn parse(src: &str) -> SteamGenres {
        let mut g = SteamGenres::default();
        for line in src.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut it = line.splitn(3, '\t');
            let (Some(id), Some(code), Some(name)) = (it.next(), it.next(), it.next()) else { continue };
            let Some(genre) = Genre::from_code(code) else { continue };
            if let Ok(id) = id.parse() {
                g.appid.insert(id, genre);
            }
            let key = name_key(name);
            if key.len() >= 3 {
                g.name.entry(key).or_insert(genre);
            }
        }
        g
    }

    pub fn by_appid(&self, id: u32) -> Option<Genre> {
        self.appid.get(&id).copied()
    }

    pub fn by_name(&self, name: &str) -> Option<Genre> {
        self.name.get(&name_key(name)).copied()
    }

    pub fn len(&self) -> usize {
        self.appid.len()
    }
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

    fn run(exe: &str, fullscreen: bool, coverage: f32, gfx: Option<bool>, lib: &[Installed], prefs: &Prefs) -> Verdict {
        let tags = SteamGenres::parse("999999\tracing\tSome Indie Game\n578080\tshooter\tPUBG: BATTLEGROUNDS\n");
        classify(exe, fullscreen, coverage, &|| gfx, &GameDb::builtin(), &tags, lib, prefs)
    }

    fn info(v: Verdict) -> GameInfo {
        match v {
            Verdict::Game(g) => g,
            other => panic!("expected a game, got {other:?}"),
        }
    }

    #[test]
    fn classifies_known_library_folder_fullscreen_and_non_games() {
        let p = Prefs::default();
        let cs = info(run(r"D:\SteamLibrary\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe", false, 1.0, None, &[], &p));
        assert_eq!((cs.name.as_str(), cs.genre, cs.source), ("Counter-Strike 2", Some(Genre::Shooter), Source::Known));

        // Generic Java only counts as Minecraft inside a Minecraft folder.
        assert_eq!(run(r"C:\Program Files\Java\bin\javaw.exe", false, 0.3, Some(false), &[], &p), Verdict::NotGame);
        let mc = info(run(r"C:\Users\a\AppData\Roaming\.minecraft\runtime\bin\javaw.exe", false, 0.5, None, &[], &p));
        assert_eq!(mc.genre, Some(Genre::Sandbox));

        let lib = vec![Installed {
            name: "Some Indie Game".into(),
            dir: PathBuf::from(r"E:\Games\Indie"),
            store: library::Store::Gog,
            steam_appid: None,
            exe: None,
        }];
        let indie = info(run(r"E:\Games\Indie\bin\indie.exe", false, 0.5, None, &lib, &p));
        assert_eq!((indie.name.as_str(), indie.genre, indie.source), ("Some Indie Game", Some(Genre::Racing), Source::Library), "genre from Steam tags by name");

        // Well-known folder without any library entry.
        let x = info(run(r"C:\XboxGames\PUBG BATTLEGROUNDS\Content\TslGame_x.exe", false, 0.5, None, &[], &p));
        assert_eq!((x.name.as_str(), x.source), ("PUBG BATTLEGROUNDS", Source::Library));
        assert_eq!(x.genre, Some(Genre::Shooter), "name match ignores punctuation");

        assert_eq!(run(r"C:\Windows\explorer.exe", true, 1.0, Some(true), &lib, &p), Verdict::NotGame);
        assert_eq!(run(r"C:\Riot Games\Riot Client\RiotClientServices.exe", true, 1.0, Some(true), &lib, &p), Verdict::NotGame);
        assert_eq!(run(r"C:\Tools\notepad.exe", false, 0.5, Some(false), &lib, &p), Verdict::NotGame);
        let fs = info(run(r"C:\Games2\cool_game-win64.exe", true, 1.0, None, &lib, &p));
        assert_eq!((fs.name.as_str(), fs.source), ("Cool", Source::Fullscreen));
    }

    #[test]
    fn windowed_3d_apps_are_asked_about_once() {
        let mut p = Prefs::default();
        let exe = r"C:\Indie\pixel.exe";
        assert!(matches!(run(exe, false, 0.8, Some(true), &[], &p), Verdict::Ask(_)));
        assert_eq!(run(exe, false, 0.3, Some(true), &[], &p), Verdict::NotGame, "small windows are ignored");
        assert_eq!(run(exe, false, 0.8, None, &[], &p), Verdict::NotGame, "unknown module list is not enough");
        p.confirmed.insert("pixel.exe".into());
        assert_eq!(info(run(exe, false, 0.2, None, &[], &p)).source, Source::Confirmed);
        p.confirmed.clear();
        p.rejected.insert("pixel.exe".into());
        assert_eq!(run(exe, true, 1.0, Some(true), &[], &p), Verdict::NotGame, "rejected even when fullscreen");
        p.rejected.clear();
        p.genres.insert("pixel.exe".into(), Genre::Racing);
        assert_eq!(info(run(exe, false, 0.2, None, &[], &p)).genre, Some(Genre::Racing));
    }

    #[test]
    fn emulators_are_games_without_genre() {
        let e = info(run(r"C:\Emu\Ryujinx.exe", false, 0.5, None, &[], &Prefs::default()));
        assert_eq!((e.source, e.genre), (Source::Known, None));
    }
}
