use std::sync::{Arc, Weak};

use kumo_core::call as core;

use crate::desktop::Presence;
use crate::{AppEvent, AppListener, KumoApp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CallSource {
    Office,
    Desktop,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CallFailure {
    TakenOver,
    TokenRetriesExhausted,
    DesktopNotInCall,
    RelayUnreachable,
    Media { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CallPhase {
    Idle,
    Waiting,
    Joining,
    Joined,
    Failed { reason: CallFailure },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CallTile {
    pub key: String,
    pub participant_id: Option<String>,
    pub name: String,
    pub avatar: String,
    pub presence: Presence,
    pub is_local: bool,
    pub in_call: bool,
    pub mic_on: bool,
    pub has_camera: bool,
    pub has_screen: bool,
    pub speaking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CallView {
    pub phase: CallPhase,
    pub source: Option<CallSource>,
    pub room_name: String,
    pub mic_on: bool,
    pub camera_on: bool,
    pub tiles: Vec<CallTile>,
    pub active_speaker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CallMediaParticipant {
    pub id: String,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub is_local: bool,
    pub mic_on: bool,
    pub has_camera: bool,
    pub has_screen: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CallAudioLevel {
    pub participant_id: String,
    pub level: f32,
}

#[uniffi::export(with_foreign)]
pub trait CallMedia: Send + Sync {
    fn join(
        &self,
        attempt: u64,
        url: String,
        token: String,
        mic: bool,
        camera: bool,
        sink: Arc<CallMediaSink>,
    );
    fn leave(&self, attempt: u64);
    fn set_inputs(&self, attempt: u64, mic: bool, camera: bool);
    fn subscribe(&self, attempt: u64, participant_ids: Vec<String>);
}

#[derive(uniffi::Object)]
pub struct CallMediaSink {
    sink: core::MediaSink,
}

#[uniffi::export]
impl CallMediaSink {
    pub fn attempt(&self) -> u64 {
        self.sink.attempt()
    }

    pub fn joined(&self) {
        self.sink.joined();
    }

    pub fn join_failed(&self, code: String, message: String) {
        self.sink.join_failed(code, message);
    }

    pub fn left(&self) {
        self.sink.left();
    }

    pub fn error(&self, message: String) {
        self.sink.error(message);
    }

    pub fn participants(&self, participants: Vec<CallMediaParticipant>) {
        self.sink
            .participants(participants.into_iter().map(Into::into).collect());
    }

    pub fn audio_levels(&self, levels: Vec<CallAudioLevel>) {
        self.sink.audio_levels(
            levels
                .into_iter()
                .map(|l| core::AudioLevel {
                    participant_id: l.participant_id,
                    level: l.level,
                })
                .collect(),
        );
    }
}

#[uniffi::export]
pub fn call_failure_label(failure: CallFailure) -> String {
    match failure {
        CallFailure::TakenOver => "Another device took over this call".into(),
        CallFailure::TokenRetriesExhausted => "Could not join the call".into(),
        CallFailure::DesktopNotInCall => "The desktop app is not in a call".into(),
        CallFailure::RelayUnreachable => "The relay is not reachable".into(),
        CallFailure::Media { detail } => detail,
    }
}

pub struct MediaAdapter(pub Arc<dyn CallMedia>);

impl core::MediaPort for MediaAdapter {
    fn join(&self, request: core::MediaJoin, sink: core::MediaSink) {
        self.0.join(
            request.attempt,
            request.url,
            request.token,
            request.mic,
            request.camera,
            Arc::new(CallMediaSink { sink }),
        );
    }

    fn leave(&self, attempt: u64) {
        self.0.leave(attempt);
    }

    fn set_inputs(&self, attempt: u64, mic: bool, camera: bool) {
        self.0.set_inputs(attempt, mic, camera);
    }

    fn subscribe(&self, attempt: u64, participant_ids: Vec<String>) {
        self.0.subscribe(attempt, participant_ids);
    }
}

pub struct ChangesAdapter(pub Arc<dyn AppListener>);

impl core::CallChanges for ChangesAdapter {
    fn call_changed(&self) {
        self.0.on_event(AppEvent::CallChanged);
    }
}

pub struct OfficeSource(pub Weak<KumoApp>);

impl core::OfficeCallSource for OfficeSource {
    fn credentials(&self) -> Option<core::Credentials> {
        self.0
            .upgrade()?
            .space_state()
            .and_then(|s| s.call_credentials())
    }

    fn zone(&self) -> core::CallZone {
        self.0
            .upgrade()
            .and_then(|app| app.space_state())
            .map(|s| s.call_zone())
            .unwrap_or_default()
    }

    fn refresh_token(&self) {
        if let Some(app) = self.0.upgrade() {
            app.refresh_office_token();
        }
    }

    fn leave(&self) {
        if let Some(app) = self.0.upgrade() {
            app.leave_office();
        }
    }
}

impl From<CallMediaParticipant> for core::MediaParticipant {
    fn from(p: CallMediaParticipant) -> Self {
        Self {
            id: p.id,
            user_id: p.user_id,
            user_name: p.user_name,
            is_local: p.is_local,
            mic_on: p.mic_on,
            has_camera: p.has_camera,
            has_screen: p.has_screen,
        }
    }
}

impl From<core::CallSource> for CallSource {
    fn from(s: core::CallSource) -> Self {
        match s {
            core::CallSource::Office => CallSource::Office,
            core::CallSource::Desktop => CallSource::Desktop,
        }
    }
}

impl From<core::CallFailure> for CallFailure {
    fn from(f: core::CallFailure) -> Self {
        match f {
            core::CallFailure::TakenOver => CallFailure::TakenOver,
            core::CallFailure::TokenRetriesExhausted => CallFailure::TokenRetriesExhausted,
            core::CallFailure::DesktopNotInCall => CallFailure::DesktopNotInCall,
            core::CallFailure::RelayUnreachable => CallFailure::RelayUnreachable,
            core::CallFailure::Media { detail } => CallFailure::Media { detail },
        }
    }
}

impl From<core::CallPhase> for CallPhase {
    fn from(p: core::CallPhase) -> Self {
        match p {
            core::CallPhase::Idle => CallPhase::Idle,
            core::CallPhase::Waiting => CallPhase::Waiting,
            core::CallPhase::Joining => CallPhase::Joining,
            core::CallPhase::Joined => CallPhase::Joined,
            core::CallPhase::Failed { reason } => CallPhase::Failed {
                reason: reason.into(),
            },
        }
    }
}

impl From<core::CallTile> for CallTile {
    fn from(t: core::CallTile) -> Self {
        Self {
            key: t.key,
            participant_id: t.participant_id,
            name: t.name,
            avatar: t.avatar,
            presence: t.presence.into(),
            is_local: t.is_local,
            in_call: t.in_call,
            mic_on: t.mic_on,
            has_camera: t.has_camera,
            has_screen: t.has_screen,
            speaking: t.speaking,
        }
    }
}

impl From<core::CallView> for CallView {
    fn from(v: core::CallView) -> Self {
        Self {
            phase: v.phase.into(),
            source: v.source.map(Into::into),
            room_name: v.room_name,
            mic_on: v.mic_on,
            camera_on: v.camera_on,
            tiles: v.tiles.into_iter().map(Into::into).collect(),
            active_speaker: v.active_speaker,
        }
    }
}
