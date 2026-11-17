use std::sync::Arc;

use futures_util::future::BoxFuture;
use tokio::sync::mpsc;

use super::machine::{Input, MediaJoin};
use super::types::{
    AudioLevel, CallZone, Credentials, Grant, HandoffFailure, MediaEvent, MediaParticipant,
    RenewOutcome,
};

pub trait MediaPort: Send + Sync {
    fn join(&self, request: MediaJoin, sink: MediaSink);
    fn leave(&self, attempt: u64);
    fn set_inputs(&self, attempt: u64, mic: bool, camera: bool);
    fn subscribe(&self, attempt: u64, participant_ids: Vec<String>);
}

pub trait OfficeCallSource: Send + Sync {
    fn credentials(&self) -> Option<Credentials>;
    fn zone(&self) -> CallZone;
    fn refresh_token(&self);
    fn leave(&self);
}

pub trait RelayCalls: Send + Sync {
    fn handoff(&self) -> BoxFuture<'static, Result<Grant, HandoffFailure>>;
    fn renew(&self, lease_id: String) -> BoxFuture<'static, RenewOutcome>;
    fn reclaim(&self, lease_id: String) -> BoxFuture<'static, ()>;
    fn zone(&self) -> BoxFuture<'static, Option<CallZone>>;
}

pub trait CallChanges: Send + Sync {
    fn call_changed(&self);
}

#[derive(Clone)]
pub struct CallPorts {
    pub media: Arc<dyn MediaPort>,
    pub office: Arc<dyn OfficeCallSource>,
    pub relay: Arc<dyn RelayCalls>,
    pub changes: Arc<dyn CallChanges>,
}

#[derive(Clone)]
pub struct MediaSink {
    attempt: u64,
    inputs: mpsc::UnboundedSender<Input>,
}

impl MediaSink {
    pub(super) fn new(attempt: u64, inputs: mpsc::UnboundedSender<Input>) -> Self {
        Self { attempt, inputs }
    }

    pub fn attempt(&self) -> u64 {
        self.attempt
    }

    pub fn send(&self, event: MediaEvent) {
        let _ = self.inputs.send(Input::Media {
            attempt: self.attempt,
            event,
        });
    }

    pub fn joined(&self) {
        self.send(MediaEvent::Joined);
    }

    pub fn join_failed(&self, code: String, message: String) {
        self.send(MediaEvent::JoinFailed { code, message });
    }

    pub fn left(&self) {
        self.send(MediaEvent::Left);
    }

    pub fn error(&self, message: String) {
        self.send(MediaEvent::Error { message });
    }

    pub fn participants(&self, participants: Vec<MediaParticipant>) {
        self.send(MediaEvent::Participants(participants));
    }

    pub fn audio_levels(&self, levels: Vec<AudioLevel>) {
        self.send(MediaEvent::AudioLevels(levels));
    }
}
