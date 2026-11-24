use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::events::Who;

pub const AGENT_ACCESS_FILE: &str = "agents.json";
pub const AI_NOTE_FILE: &str = "ai-note.json";
pub const AUTO_UPDATE_FILE: &str = "auto-update.json";
pub const DEFAULT_AI_NOTE: &str = "🤖 _Sent by my AI assistant_";

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
        config_path(AGENT_ACCESS_FILE)
    }

    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or(Self { enabled: false }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(_) => Self { enabled: false },
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string(self).map_err(|e| CoreError::Storage(e.to_string()))?;
        save_atomically(path, &text)
    }
}

/// Whether Better Kumo installs a new release by itself. Off unless the user turns it on, also when
/// the file is unreadable: an update restarts the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AutoUpdate {
    pub enabled: bool,
}

impl AutoUpdate {
    pub fn default_path() -> PathBuf {
        config_path(AUTO_UPDATE_FILE)
    }

    pub fn path_beside(agents_path: &Path) -> PathBuf {
        agents_path.with_file_name(AUTO_UPDATE_FILE)
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string(self).map_err(|e| CoreError::Storage(e.to_string()))?;
        save_atomically(path, &text)
    }
}

/// A line kumo-mcp adds to the messages an agent sends, so colleagues know
/// the user did not type them. Off unless the user turns it on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiNote {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_note")]
    pub text: String,
    #[serde(default, deserialize_with = "exclusions_or_none")]
    pub exclude: Vec<Who>,
}

/// A broken entry in a hand-edited file must not fail the whole note, which would turn it off,
/// nor drop the valid entries next to it.
fn exclusions_or_none<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<Who>, D::Error> {
    let entries = match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(entries) => entries,
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value(entry).ok())
        .collect())
}

fn default_note() -> String {
    DEFAULT_AI_NOTE.into()
}

impl Default for AiNote {
    fn default() -> Self {
        Self {
            enabled: false,
            text: default_note(),
            exclude: Vec::new(),
        }
    }
}

impl AiNote {
    /// Kept next to the AI agents switch, wherever that lives.
    pub fn path_beside(agents_path: &Path) -> PathBuf {
        agents_path.with_file_name(AI_NOTE_FILE)
    }

    pub fn load(path: &Path) -> Self {
        Self::try_load(path).unwrap_or_default()
    }

    /// Like [`Self::load`], but a file that exists and cannot be read or parsed is an error,
    /// so a read-modify-write does not replace it with defaults.
    pub fn try_load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<Self>(&text)
                .map(|note| note.cleaned())
                .map_err(|e| CoreError::Storage(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::Storage(e.to_string())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string(&self.cleaned())
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        save_atomically(path, &text)
    }

    pub fn cleaned(&self) -> Self {
        let text = self.text.trim();
        let mut seen = std::collections::HashSet::new();
        let mut exclude: Vec<Who> = Vec::with_capacity(self.exclude.len());
        for who in &self.exclude {
            let user_id = who.user_id.trim();
            if !user_id.is_empty() && seen.insert(user_id) {
                exclude.push(Who {
                    user_id: user_id.into(),
                    name: who.name.trim().into(),
                });
            }
        }
        Self {
            enabled: self.enabled,
            text: if text.is_empty() {
                default_note()
            } else {
                text.into()
            },
            exclude,
        }
    }

    /// No recipients means unknown readers, and an unknown reader may not be excluded.
    pub fn applies_to<'a>(&self, recipients: impl IntoIterator<Item = &'a str>) -> bool {
        if !self.enabled {
            return false;
        }
        let mut any = false;
        for id in recipients {
            any = true;
            if !self.exclude.iter().any(|w| w.user_id == id) {
                return true;
            }
        }
        !any
    }

    pub fn apply_to<'a>(
        &self,
        recipients: impl IntoIterator<Item = &'a str>,
        message: &str,
    ) -> String {
        if self.applies_to(recipients) {
            self.apply(message)
        } else {
            message.into()
        }
    }

    /// The message as it goes out: with the note at the end when it is on.
    /// Adding it twice (an edit of a message that already has it) is a no-op.
    pub fn apply(&self, message: &str) -> String {
        if !self.enabled {
            return message.into();
        }
        let note = match self.text.trim() {
            "" => DEFAULT_AI_NOTE,
            text => text,
        };
        let message = message.trim_end();
        if message.is_empty() {
            note.into()
        } else if message.ends_with(note) {
            message.into()
        } else {
            format!("{message}\n\n{note}")
        }
    }
}

pub(crate) fn config_path(file: &str) -> PathBuf {
    std::env::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("kumowatch")
        .join(file)
}

pub(crate) fn save_atomically(path: &Path, text: &str) -> Result<()> {
    let storage = |e: std::io::Error| CoreError::Storage(e.to_string());
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(storage)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.{}.{}.tmp",
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
    fn the_ai_note_is_off_by_default_and_added_once() {
        let dir = temp_dir("note");
        let path = AiNote::path_beside(&dir.join(AGENT_ACCESS_FILE));
        let note = AiNote::load(&path);
        assert!(!note.enabled);
        assert_eq!(note.apply("hi"), "hi");
        AiNote {
            enabled: true,
            text: "  (via AI)  ".into(),
            exclude: Vec::new(),
        }
        .save(&path)
        .unwrap();
        let note = AiNote::load(&path);
        assert_eq!(note.text, "(via AI)");
        assert_eq!(note.apply("hi\n"), "hi\n\n(via AI)");
        assert_eq!(note.apply("hi\n\n(via AI)"), "hi\n\n(via AI)");
        assert_eq!(note.apply(""), "(via AI)");
        let blank = AiNote {
            enabled: true,
            text: " ".into(),
            exclude: Vec::new(),
        };
        assert_eq!(blank.apply("hi"), format!("hi\n\n{DEFAULT_AI_NOTE}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn excluded_people_get_messages_without_the_note() {
        let who = |id: &str| Who {
            user_id: id.into(),
            name: format!(" {id} "),
        };
        let note = AiNote {
            enabled: true,
            text: "(via AI)".into(),
            exclude: vec![who(" amy "), who("bob"), who("amy"), who(" ")],
        }
        .cleaned();
        assert_eq!(
            note.exclude,
            vec![
                Who {
                    user_id: "amy".into(),
                    name: "amy".into()
                },
                Who {
                    user_id: "bob".into(),
                    name: "bob".into()
                },
            ]
        );
        assert_eq!(note.apply_to(["amy"], "hi"), "hi");
        assert_eq!(note.apply_to(["amy", "bob"], "hi"), "hi");
        assert_eq!(note.apply_to(["amy", "cat"], "hi"), "hi\n\n(via AI)");
        assert_eq!(note.apply_to(["cat"], "hi"), "hi\n\n(via AI)");
        assert_eq!(note.apply_to([], "hi"), "hi\n\n(via AI)");
        let off = AiNote {
            enabled: false,
            ..note
        };
        assert_eq!(off.apply_to(["cat"], "hi"), "hi");
    }

    #[test]
    fn a_hand_edited_note_file_is_cleaned_on_load() {
        let dir = temp_dir("hand-edited-note");
        let path = AiNote::path_beside(&dir.join(AGENT_ACCESS_FILE));
        std::fs::write(
            &path,
            r#"{"enabled":true,"text":"x","exclude":[{"user_id":" amy "}]}"#,
        )
        .unwrap();
        assert_eq!(AiNote::load(&path).exclude[0].user_id, "amy");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_file_from_before_exclusions_still_loads() {
        let dir = temp_dir("old-note");
        let path = AiNote::path_beside(&dir.join(AGENT_ACCESS_FILE));
        std::fs::write(&path, r#"{"enabled":true,"text":"x"}"#).unwrap();
        let note = AiNote::load(&path);
        assert!(note.enabled);
        assert!(note.exclude.is_empty());
        std::fs::write(
            &path,
            r#"{"enabled":true,"exclude":[{"user_id":"amy"},{"user_id":"bob","name":null},"cat"]}"#,
        )
        .unwrap();
        let ids: Vec<String> = AiNote::load(&path)
            .exclude
            .into_iter()
            .map(|w| w.user_id)
            .collect();
        assert_eq!(ids, ["amy"]);
        std::fs::write(&path, r#"{"enabled":true,"text":null}"#).unwrap();
        assert!(AiNote::try_load(&path).is_err());
        assert!(!AiNote::load(&path).enabled);
        for broken in [r#"null"#, r#"["amy"]"#, r#"{"user_id":"amy"}"#] {
            std::fs::write(
                &path,
                format!(r#"{{"enabled":true,"text":"x","exclude":{broken}}}"#),
            )
            .unwrap();
            let note = AiNote::load(&path);
            assert!(note.enabled, "{broken}");
            assert!(note.exclude.is_empty(), "{broken}");
        }
        let _ = std::fs::remove_dir_all(&dir);
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
        let path = dir.join(AUTO_UPDATE_FILE);
        assert!(!AutoUpdate::load(&path).enabled);
        AutoUpdate { enabled: true }.save(&path).unwrap();
        assert!(AutoUpdate::load(&path).enabled);
        std::fs::write(&path, "not json").unwrap();
        assert!(!AutoUpdate::load(&path).enabled);
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
