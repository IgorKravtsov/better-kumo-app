pub mod audio;
pub mod config;
pub mod detector;
mod list;
pub mod model;
pub mod module;
pub mod page;
pub mod recording;
mod service;
pub mod storage;
pub mod transcript;

pub use config::MeetingsConfig;
pub use list::{MeetingEntry, MeetingState};
pub use module::{Module, ModuleState, ProcessTranscriber, Transcribe};
pub use page::MeetingPage;
pub use service::{ANNOUNCEMENT, Activity, Meetings, MeetingsDeps, MeetingsStatus};
pub use storage::AudioUsage;
