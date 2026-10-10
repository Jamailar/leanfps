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

    /// `None` if the file is missing or unreadable. A file that exists but can't be parsed is
    /// moved aside to `<name>.corrupt-<time>` first, so the next save can't overwrite the
    /// user's original values (the journal) without a copy being kept.
    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let path = self.path(name);
        let bytes = std::fs::read(&path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(v) => Some(v),
            Err(_) => {
                let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let _ = std::fs::rename(&path, self.path(&format!("{name}.corrupt-{secs}")));
                None
            }
        }
    }

    /// Write atomically: temp file, flushed to disk, then rename, so a crash or power cut never
    /// leaves half (or an empty) journal.
    pub fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        let tmp = self.path(&format!("{name}.tmp"));
        let data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        let write = || -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()
        };
        write().map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, self.path(name)).map_err(|e| format!("cannot save {name}: {e}"))
    }

    /// Only ever removes one of LeanFPS's own files inside the data directory.
    pub fn remove(&self, name: &str) {
        let _ = std::fs::remove_file(self.path(name));
    }
}
