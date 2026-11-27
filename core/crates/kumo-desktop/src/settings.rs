use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const KEEP_KUMOSPACE_OPEN: &str = "keep_kumospace_open";
const SETUP_GUIDE_DONE: &str = "setup_guide_done";

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
    set(path, KEEP_KUMOSPACE_OPEN, keep)
}

pub fn set_setup_guide_done(path: &Path) -> anyhow::Result<()> {
    set(path, SETUP_GUIDE_DONE, true)
}

/// Whether the Setup guide opens by itself. The first run writes `false`, so the relay.toml that
/// run creates is not mistaken for a computer set up before the guide existed, which is marked done.
pub fn guide_at_start(path: &Path, relay_configured: bool) -> anyhow::Result<bool> {
    match read(path).get(SETUP_GUIDE_DONE).and_then(Value::as_bool) {
        Some(done) => Ok(!done),
        None if relay_configured => set(path, SETUP_GUIDE_DONE, true).map(|()| false),
        None => set(path, SETUP_GUIDE_DONE, false).map(|()| true),
    }
}

fn set(path: &Path, key: &str, value: bool) -> anyhow::Result<()> {
    let mut settings = read(path);
    settings[key] = Value::Bool(value);
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
    fn the_guide_opens_by_itself_until_it_is_done() {
        let path = temp_path("guide-new");
        assert!(guide_at_start(&path, false).unwrap(), "a new computer");
        assert!(
            guide_at_start(&path, true).unwrap(),
            "interrupted after its own first run wrote relay.toml"
        );
        set_setup_guide_done(&path).unwrap();
        assert!(!guide_at_start(&path, true).unwrap());
    }

    #[test]
    fn a_computer_set_up_before_the_guide_never_sees_it_unasked() {
        let older = temp_path("guide-older");
        assert!(!guide_at_start(&older, true).unwrap());
        assert!(
            !guide_at_start(&older, false).unwrap(),
            "remembered, even if relay.toml goes away"
        );
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
