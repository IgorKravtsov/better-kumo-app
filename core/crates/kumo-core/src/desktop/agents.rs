use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

pub const AGENT_ACCESS_FILE: &str = "agents.json";

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAccess {
    pub enabled: bool,
}

impl Default for AgentAccess {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl AgentAccess {
    pub fn default_path() -> PathBuf {
        std::env::home_dir()
            .unwrap_or_default()
            .join(".config")
            .join("kumowatch")
            .join(AGENT_ACCESS_FILE)
    }

    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or(Self { enabled: false }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(_) => Self { enabled: false },
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let storage = |e: std::io::Error| CoreError::Storage(e.to_string());
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(storage)?;
        let text = serde_json::to_string(self).map_err(|e| CoreError::Storage(e.to_string()))?;
        let tmp = dir.join(format!(
            ".{AGENT_ACCESS_FILE}.{}.{}.tmp",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .and_then(|mut file| {
                file.write_all(text.as_bytes())?;
                file.sync_all()
            })
            .and_then(|()| std::fs::rename(&tmp, path));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written.map_err(storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kumo-agent-access-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_means_on_and_a_broken_file_means_off() {
        let dir = temp_dir("load");
        let path = dir.join(AGENT_ACCESS_FILE);
        assert!(AgentAccess::load(&path).enabled);
        AgentAccess { enabled: false }.save(&path).unwrap();
        assert!(!AgentAccess::load(&path).enabled);
        AgentAccess { enabled: true }.save(&path).unwrap();
        assert!(AgentAccess::load(&path).enabled);
        std::fs::write(&path, "not json").unwrap();
        assert!(!AgentAccess::load(&path).enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_saves_always_leave_a_valid_file() {
        let dir = temp_dir("race");
        let path = dir.join(AGENT_ACCESS_FILE);
        let writers: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        AgentAccess {
                            enabled: i % 2 == 0,
                        }
                        .save(&path)
                        .unwrap();
                        let text = std::fs::read_to_string(&path).unwrap();
                        serde_json::from_str::<AgentAccess>(&text).unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(leftovers, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
