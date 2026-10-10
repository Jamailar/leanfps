//! User settings and license tier.

use crate::catalog::{Genre, Tweak};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const TRIAL_SESSIONS: u32 = 3;
pub const PRICE: &str = "$19";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub preset: String,
    /// Tweaks the user switched away from the preset.
    pub overrides: BTreeMap<String, bool>,
    /// Genre the user picked for a game, by lower-case exe file name.
    pub user_genres: HashMap<String, Genre>,
    /// Apps the user confirmed are / are not games, by lower-case exe file name.
    pub confirmed_games: HashSet<String>,
    pub not_games: HashSet<String>,
    /// Game sessions started so far; the first few get every Pro feature.
    pub sessions_played: u32,
    /// A verified Pro license is on this PC (`license.json`, checked in `license`). Never
    /// saved: it is worked out again from the signed file on every start.
    #[serde(skip)]
    pub pro: bool,
    pub first_run_done: bool,
    pub always_on_top: bool,
    /// Closing the window keeps LeanFPS running in the tray.
    pub close_to_tray: bool,
    pub start_with_windows: bool,
    /// "system", "light" or "dark".
    pub theme: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            preset: "recommended".into(),
            overrides: BTreeMap::new(),
            user_genres: HashMap::new(),
            confirmed_games: HashSet::new(),
            not_games: HashSet::new(),
            sessions_played: 0,
            pro: false,
            first_run_done: false,
            always_on_top: false,
            close_to_tray: true,
            start_with_windows: false,
            theme: "system".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Free,
    Trial { left: u32 },
    Pro,
}

impl Config {
    pub fn wants(&self, t: &Tweak) -> bool {
        self.overrides.get(&t.id).copied().unwrap_or_else(|| t.in_preset(&self.preset))
    }

    pub fn is_custom(&self) -> bool {
        !self.overrides.is_empty()
    }

    pub fn set_preset(&mut self, preset: &str) {
        self.preset = preset.into();
        self.overrides.clear();
    }

    pub fn set_wanted(&mut self, t: &Tweak, on: bool) {
        if t.in_preset(&self.preset) == on {
            self.overrides.remove(&t.id);
        } else {
            self.overrides.insert(t.id.clone(), on);
        }
    }

    pub fn prefs(&self) -> crate::games::Prefs {
        crate::games::Prefs { genres: self.user_genres.clone(), confirmed: self.confirmed_games.clone(), rejected: self.not_games.clone() }
    }

    pub fn tier(&self) -> Tier {
        if self.pro {
            Tier::Pro
        } else if self.sessions_played < TRIAL_SESSIONS {
            Tier::Trial { left: TRIAL_SESSIONS - self.sessions_played }
        } else {
            Tier::Free
        }
    }

    /// Wanted and allowed by the current tier.
    pub fn active(&self, t: &Tweak) -> bool {
        self.wants(t) && (!t.pro || self.tier() != Tier::Free)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    #[test]
    fn overrides_and_tiers() {
        let cat = Catalog::builtin();
        let mut c = Config::default();
        let dvr = cat.get("game_dvr_off").unwrap();
        let hags = cat.get("hags_on").unwrap();
        assert!(c.wants(dvr) && !c.wants(hags));
        c.set_wanted(hags, true);
        c.set_wanted(dvr, true);
        assert!(c.is_custom());
        assert_eq!(c.overrides.len(), 1, "matching the preset is not an override");
        assert_eq!(c.tier(), Tier::Trial { left: 3 });
        assert!(c.active(hags));
        c.sessions_played = 3;
        assert_eq!(c.tier(), Tier::Free);
        assert!(!c.active(hags), "pro tweak inactive on free tier");
        assert!(c.active(dvr));
        c.pro = true;
        assert_eq!(c.tier(), Tier::Pro);
        assert!(c.active(hags));
        let saved = serde_json::to_string(&c).unwrap();
        assert!(!serde_json::from_str::<Config>(&saved).unwrap().pro, "Pro is never read from the settings file");
        c.set_preset("competitive");
        assert!(!c.is_custom());
    }
}
