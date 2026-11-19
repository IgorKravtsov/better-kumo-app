use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::agents::{config_path, save_atomically};
use crate::error::{CoreError, Result};

pub const WATCH_LIST_FILE: &str = "watch.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedPerson {
    pub user_id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchList {
    #[serde(default)]
    pub people: Vec<WatchedPerson>,
}

impl WatchList {
    pub fn default_path() -> PathBuf {
        config_path(WATCH_LIST_FILE)
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(&self.cleaned())
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        save_atomically(path, &text)
    }

    pub fn contains(&self, user_id: &str) -> bool {
        self.people.iter().any(|p| p.user_id == user_id)
    }

    pub fn cleaned(&self) -> Self {
        let mut people: Vec<WatchedPerson> = Vec::new();
        for person in &self.people {
            let user_id = person.user_id.trim();
            if user_id.is_empty() || people.iter().any(|p| p.user_id == user_id) {
                continue;
            }
            people.push(WatchedPerson {
                user_id: user_id.to_string(),
                name: person.name.trim().to_string(),
            });
        }
        Self { people }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(user_id: &str, name: &str) -> WatchedPerson {
        WatchedPerson {
            user_id: user_id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn missing_or_broken_file_watches_no_one() {
        let dir = std::env::temp_dir().join(format!(
            "kumo-watch-list-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join(WATCH_LIST_FILE);
        assert_eq!(WatchList::load(&path), WatchList::default());
        WatchList {
            people: vec![
                person("amy", "Amy"),
                person(" amy ", "Amy again"),
                person("", "x"),
            ],
        }
        .save(&path)
        .unwrap();
        let loaded = WatchList::load(&path);
        assert_eq!(loaded.people, vec![person("amy", "Amy")]);
        assert!(loaded.contains("amy"));
        assert!(!loaded.contains("bo"));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(WatchList::load(&path), WatchList::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
