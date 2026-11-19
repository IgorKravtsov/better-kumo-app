use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const KEEP_KUMOSPACE_OPEN: &str = "keep_kumospace_open";

pub fn path() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("kumowatch")
        .join("better-kumo.json")
}

fn read(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

pub fn keep_kumospace_open(path: &Path) -> bool {
    read(path)
        .get(KEEP_KUMOSPACE_OPEN)
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

pub fn set_keep_kumospace_open(path: &Path, keep: bool) -> anyhow::Result<()> {
    let mut settings = read(path);
    settings[KEEP_KUMOSPACE_OPEN] = Value::Bool(keep);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_string_pretty(&settings)?)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "better-kumo-settings-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("better-kumo.json")
    }

    #[test]
    fn keeps_kumospace_open_by_default() {
        assert!(keep_kumospace_open(&temp_path("default")));
    }

    #[test]
    fn remembers_the_choice() {
        let path = temp_path("choice");
        set_keep_kumospace_open(&path, false).unwrap();
        assert!(!keep_kumospace_open(&path));
        set_keep_kumospace_open(&path, true).unwrap();
        assert!(keep_kumospace_open(&path));
    }

    #[test]
    fn keeps_other_keys() {
        let path = temp_path("other");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"other": 1}"#).unwrap();
        set_keep_kumospace_open(&path, false).unwrap();
        assert_eq!(read(&path)["other"], json!(1));
        assert!(!path.with_extension("json.tmp").exists());
    }
}
