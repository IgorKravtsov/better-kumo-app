pub mod audio;
pub mod config;
pub mod detector;
pub mod model;
pub mod module;
pub mod page;
pub mod recording;
mod service;
pub mod transcript;

pub use config::MeetingsConfig;
pub use module::{Module, ModuleState, ProcessTranscriber, Transcribe};
pub use page::MeetingPage;
pub use service::{ANNOUNCEMENT, Activity, Meetings, MeetingsDeps, MeetingsStatus};
