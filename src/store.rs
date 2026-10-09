//! JSON files in the data directory (`%ProgramData%\LeanFPS` on Windows).

use serde::{de::DeserializeOwned, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir }
    }

    /// `LEANFPS_DATA_DIR`, else `%ProgramData%\LeanFPS`, else `./leanfps-data`.
    pub fn default_location() -> Store {
        if let Ok(d) = std::env::var("LEANFPS_DATA_DIR") {
            return Store::new(d.into());
        }
        if let Ok(pd) = std::env::var("ProgramData") {
            return Store::new(PathBuf::from(pd).join("LeanFPS"));
        }
        Store::new(PathBuf::from("leanfps-data"))
    }

    #[cfg(test)]
    pub fn temp() -> Store {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("leanfps-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        Store::new(dir)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let bytes = std::fs::read(self.path(name)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write atomically: temp file then rename, so a crash never leaves half a journal.
    pub fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        let tmp = self.path(&format!("{name}.tmp"));
        let data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, data).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, self.path(name)).map_err(|e| format!("cannot save {name}: {e}"))
    }

    pub fn remove(&self, name: &str) {
        let _ = std::fs::remove_file(self.path(name));
    }
}
