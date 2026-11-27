use std::path::PathBuf;
use std::time::Duration;

use anyhow::bail;
use serde::{Deserialize, Serialize};

use crate::detector::Rules;
use crate::model::Model;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeetingsConfig {
    pub auto_start: bool,
    pub settle_seconds: u64,
    pub alone_stop_seconds: u64,
    pub quiet_stop_seconds: u64,
    pub min_speech_seconds: u64,
    pub language: String,
    pub model: String,
    pub folder: Option<PathBuf>,
    pub keep_audio: bool,
    /// Kept audio is deleted this many days after the meeting; 0 (the default, so configs from
    /// before this setting keep what they kept) keeps it.
    pub audio_days: u64,
    /// Kept audio is stored as FLAC, a fraction of the WAV size.
    pub compress_audio: bool,
    pub announce: bool,
    pub transcriber: Option<PathBuf>,
}

impl Default for MeetingsConfig {
    fn default() -> Self {
        let rules = Rules::default();
        Self {
            auto_start: false,
            settle_seconds: rules.settle.as_secs(),
            alone_stop_seconds: rules.alone_stop.as_secs(),
            quiet_stop_seconds: rules.quiet_stop.as_secs(),
            min_speech_seconds: rules.min_speech.as_secs(),
            language: "auto".into(),
            model: "large-v3-turbo-q5_0".into(),
            folder: None,
            keep_audio: false,
            audio_days: 0,
            compress_audio: true,
            announce: true,
            transcriber: None,
        }
    }
}

impl MeetingsConfig {
    pub fn rules(&self) -> Rules {
        Rules {
            auto_start: self.auto_start,
            settle: Duration::from_secs(self.settle_seconds),
            alone_stop: Duration::from_secs(self.alone_stop_seconds),
            quiet_stop: Duration::from_secs(self.quiet_stop_seconds.max(10)),
            min_speech: Duration::from_secs(self.min_speech_seconds),
        }
    }

    pub fn validated(mut self) -> anyhow::Result<Self> {
        let within = |name: &str, value: u64, max: u64| {
            if value > max {
                bail!("{name} must be at most {max} seconds");
            }
            Ok(())
        };
        within("The settle time", self.settle_seconds, 600)?;
        within("Stop after being alone", self.alone_stop_seconds, 600)?;
        within("Stop after silence", self.quiet_stop_seconds, 3_600)?;
        within("Minimum speech", self.min_speech_seconds, 600)?;
        if self.audio_days > 3_650 {
            bail!("Delete audio after must be at most 3650 days");
        }
        if self.quiet_stop_seconds < 10 {
            bail!("Stop after silence must be at least 10 seconds");
        }
        self.language = self.language.trim().to_lowercase();
        if self.language.is_empty() {
            self.language = "auto".into();
        }
        let valid_language =
            self.language.len() <= 8 && self.language.chars().all(|c| c.is_ascii_lowercase());
        if !valid_language {
            bail!(
                "\"{}\" is not a language code like auto, ru, uk or en",
                self.language
            );
        }
        Model::named(&self.model)?;
        self.model = self.model.trim().into();
        if self
            .folder
            .as_ref()
            .is_some_and(|f| f.as_os_str().is_empty())
        {
            self.folder = None;
        }
        if let Some(folder) = &self.folder
            && !folder.is_absolute()
        {
            bail!("the meetings folder must be a full path");
        }
        Ok(self)
    }

    pub fn folder(&self) -> PathBuf {
        self.folder.clone().unwrap_or_else(|| {
            std::env::home_dir()
                .unwrap_or_default()
                .join("Documents")
                .join("Better Kumo")
                .join("Meetings")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_section_or_field_takes_the_defaults() {
        let config: MeetingsConfig = toml_like(r#"{ "settle_seconds": 8 }"#);
        assert!(!config.auto_start);
        assert!(!config.rules().auto_start);
        assert_eq!(config.rules().settle, Duration::from_secs(8));
        assert_eq!(config.rules().alone_stop, Duration::from_secs(5));
        assert_eq!(config.language, "auto");
        assert!(config.announce);
        assert_eq!(config.audio_days, 0);
        assert!(config.compress_audio);
        assert!(config.folder().ends_with("Documents/Better Kumo/Meetings"));
    }

    #[test]
    fn settings_from_the_page_are_checked() {
        let ok = MeetingsConfig {
            language: " RU ".into(),
            folder: Some(PathBuf::new()),
            ..MeetingsConfig::default()
        }
        .validated()
        .unwrap();
        assert_eq!(ok.language, "ru");
        assert_eq!(ok.folder, None);
        let bad = |config: MeetingsConfig| config.validated().is_err();
        assert!(bad(MeetingsConfig {
            settle_seconds: 601,
            ..MeetingsConfig::default()
        }));
        assert!(bad(MeetingsConfig {
            language: "ru; rm".into(),
            ..MeetingsConfig::default()
        }));
        assert!(bad(MeetingsConfig {
            model: "../x".into(),
            ..MeetingsConfig::default()
        }));
        assert!(bad(MeetingsConfig {
            folder: Some(PathBuf::from("relative")),
            ..MeetingsConfig::default()
        }));
    }

    fn toml_like(json: &str) -> MeetingsConfig {
        serde_json::from_str(json).unwrap()
    }
}
