use crate::runtime::state::RuntimeState;
use chrono::{DateTime, Utc};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
#[derive(Debug)]
pub struct RuntimeStore {
    path: PathBuf,
    last_saved: Mutex<Option<DateTime<Utc>>>,
}
impl RuntimeStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_saved: Mutex::new(None),
        }
    }
    pub fn load(&self) -> Result<Option<RuntimeState>, String> {
        match fs::read(&self.path) {
            Ok(raw) => serde_json::from_slice(&raw)
                .map(Some)
                .map_err(|e| format!("Invalid runtime state {}: {e}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!(
                "Cannot read runtime state {}: {e}",
                self.path.display()
            )),
        }
    }
    pub fn save(&self, state: &RuntimeState) -> Result<(), String> {
        self.save_checked(state, |_| Ok(()))
    }
    fn save_checked(
        &self,
        state: &RuntimeState,
        check: impl Fn(&str) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut last = self
            .last_saved
            .lock()
            .map_err(|_| "Runtime store lock poisoned".to_string())?;
        if last.is_some_and(|saved| state.last_updated < saved) {
            return Err("Refusing stale runtime snapshot".into());
        }
        let parent = self
            .path
            .parent()
            .ok_or("Runtime state has no parent directory")?;
        fs::create_dir_all(parent).map_err(|e| format!("Cannot create runtime directory: {e}"))?;
        let tmp = parent.join(format!(
            ".state-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|e| e.to_string())?;
            let json = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
            check("write")?;
            file.write_all(&json).map_err(|e| e.to_string())?;
            check("file_sync")?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            check("rename")?;
            fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
            check("dir_sync")?;
            File::open(parent)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())?;
            *last = Some(state.last_updated);
            Ok(())
        })();
        let _ = fs::remove_file(tmp);
        result.map_err(|e: String| {
            format!("Cannot commit runtime state {}: {e}", self.path.display())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn store() -> RuntimeStore {
        RuntimeStore::new(
            std::env::temp_dir()
                .join(format!(
                    "duo-store-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ))
                .join("state.json"),
        )
    }
    #[test]
    fn missing_is_initial_but_corrupt_is_not() {
        let s = store();
        assert!(s.load().unwrap().is_none());
        fs::create_dir_all(s.path.parent().unwrap()).unwrap();
        fs::write(&s.path, b"{truncated").unwrap();
        assert!(s.load().is_err());
        assert_eq!(fs::read(&s.path).unwrap(), b"{truncated");
        fs::remove_dir_all(s.path.parent().unwrap()).unwrap();
    }
    #[test]
    fn committed_snapshot_roundtrips_and_failed_replacement_preserves_it() {
        let s = store();
        let mut state = RuntimeState::default();
        state.secondary_panel_disabled_by_runtime = true;
        s.save(&state).unwrap();
        assert!(
            s.load()
                .unwrap()
                .unwrap()
                .secondary_panel_disabled_by_runtime
        );
        let old = fs::read(&s.path).unwrap();
        // A destination directory cannot be replaced by the atomic file rename.
        let blocked = s.path.parent().unwrap().join("blocked");
        fs::create_dir(&blocked).unwrap();
        assert!(RuntimeStore::new(blocked).save(&state).is_err());
        assert_eq!(fs::read(&s.path).unwrap(), old);
        fs::remove_dir_all(s.path.parent().unwrap()).unwrap();
    }
    #[test]
    fn each_failed_durability_stage_leaves_a_complete_snapshot() {
        for stage in ["write", "file_sync", "rename", "dir_sync"] {
            let s = store();
            let mut state = RuntimeState::default();
            s.save(&state).unwrap();
            state.secondary_panel_disabled_by_runtime = true;
            assert!(s
                .save_checked(&state, |at| if at == stage {
                    Err("injected I/O failure".into())
                } else {
                    Ok(())
                })
                .is_err());
            let saved = s.load().unwrap().unwrap();
            assert!(!saved.secondary_panel_disabled_by_runtime || stage == "dir_sync");
            assert_eq!(fs::read_dir(s.path.parent().unwrap()).unwrap().count(), 1);
            fs::remove_dir_all(s.path.parent().unwrap()).unwrap();
        }
    }
}
