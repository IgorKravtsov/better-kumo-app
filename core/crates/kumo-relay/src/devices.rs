use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub token: String,
    #[serde(default)]
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub registered_at: u64,
}

pub struct Devices {
    path: PathBuf,
    list: Mutex<Vec<Device>>,
}

impl Devices {
    pub fn load(path: PathBuf) -> Self {
        let list = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            path,
            list: Mutex::new(list),
        }
    }

    fn save(&self, list: &[Device]) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(list) {
            let _ = std::fs::write(&self.path, text);
        }
    }

    pub fn all(&self) -> Vec<Device> {
        self.list.lock().map(|l| l.clone()).unwrap_or_default()
    }

    pub fn enabled(&self) -> Vec<Device> {
        self.all().into_iter().filter(|d| d.enabled).collect()
    }

    pub fn upsert(&self, device: Device) {
        if let Ok(mut list) = self.list.lock() {
            match list.iter_mut().find(|d| d.token == device.token) {
                Some(existing) => *existing = device,
                None => list.push(device),
            }
            self.save(&list);
        }
    }

    pub fn remove(&self, token: &str) -> bool {
        let Ok(mut list) = self.list.lock() else {
            return false;
        };
        let before = list.len();
        list.retain(|d| d.token != token);
        let removed = list.len() != before;
        if removed {
            self.save(&list);
        }
        removed
    }
}
