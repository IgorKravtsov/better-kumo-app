use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::types::{
    AudioLevel, CallFailure, CallPhase, CallSource, CallTile, CallView, CallZone, Credentials,
    Grant, HandoffFailure, Mate, MediaEvent, MediaParticipant, RenewOutcome, is_token_problem,
};
use crate::desktop::Presence;

pub const SPEAKING_LEVEL: f32 = 0.02;
pub const MAX_RETRIES: u32 = 3;
pub const RENEW_EVERY: Duration = Duration::from_secs(10);
pub const POLL_EVERY: Duration = Duration::from_secs(3);
const GUEST: &str = "Guest";

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Media {
        attempt: u64,
        event: MediaEvent,
    },
    Credentials(Option<Credentials>),
    Zone(CallZone),
    DesktopPolled(Option<CallZone>),
    Granted(Result<Grant, HandoffFailure>),
    Renewed {
        lease_id: String,
        outcome: RenewOutcome,
    },
    Timer(Timer),
    SetMic(bool),
    SetCamera(bool),
    Leave,
    Abandon,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Timer {
    Retry { generation: u64 },
    Renew { lease_id: String },
    Poll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaJoin {
    pub attempt: u64,
    pub url: String,
    pub token: String,
    pub mic: bool,
    pub camera: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Join(MediaJoin),
    Leave {
        attempt: u64,
    },
    SetInputs {
        attempt: u64,
        mic: bool,
        camera: bool,
    },
    Subscribe {
        attempt: u64,
        participant_ids: Vec<String>,
    },
    RefreshToken,
    LeaveOffice,
    Handoff,
    Renew {
        lease_id: String,
    },
    Reclaim {
        lease_id: String,
    },
    FetchDesktop,
    Schedule {
        after: Duration,
        timer: Timer,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    Credentials,
    Backoff,
    Grant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    Waiting(Wait),
    Joining,
    Joined,
    Failed(CallFailure),
    Ended,
}

pub struct CallMachine {
    source: CallSource,
    stage: Stage,
    attempt: u64,
    media_live: bool,
    mic: bool,
    camera: bool,
    retries: u32,
    generation: u64,
    credentials: Option<Credentials>,
    joined: Option<Credentials>,
    spent_token: Option<String>,
    lease: Option<String>,
    polling: bool,
    zone: CallZone,
    participants: Vec<MediaParticipant>,
    levels: HashMap<String, f32>,
    subscribed: Option<Vec<String>>,
    active_speaker: Option<String>,
}

impl CallMachine {
    pub fn new(source: CallSource) -> Self {
        Self {
            source,
            stage: match source {
                CallSource::Office => Stage::Waiting(Wait::Credentials),
                CallSource::Desktop => Stage::Waiting(Wait::Grant),
            },
            attempt: 0,
            media_live: false,
            mic: false,
            camera: false,
            retries: 0,
            generation: 0,
            credentials: None,
            joined: None,
            spent_token: None,
            lease: None,
            polling: false,
            zone: CallZone::default(),
            participants: Vec::new(),
            levels: HashMap::new(),
            subscribed: None,
            active_speaker: None,
        }
    }

    pub fn source(&self) -> CallSource {
        self.source
    }

    pub fn start(&mut self) -> Vec<Effect> {
        match self.source {
            CallSource::Office => Vec::new(),
            CallSource::Desktop => vec![Effect::Handoff],
        }
    }

    pub fn is_live(&self) -> bool {
        !matches!(self.stage, Stage::Failed(_) | Stage::Ended)
    }

    pub fn is_waiting(&self) -> bool {
        matches!(self.stage, Stage::Waiting(_))
    }

    pub fn step(&mut self, input: Input) -> Vec<Effect> {
        let mut out = Vec::new();
        match input {
            Input::Granted(result) => self.on_granted(result, &mut out),
            _ if self.stage == Stage::Ended => {}
            Input::Media { attempt, event } => self.on_media(attempt, event, &mut out),
            Input::Credentials(credentials) => self.on_credentials(credentials, &mut out),
            Input::Zone(zone) => {
                if self.source == CallSource::Office {
                    self.apply_zone(zone);
                }
            }
            Input::DesktopPolled(zone) => self.on_polled(zone, &mut out),
            Input::Renewed { lease_id, outcome } => self.on_renewed(lease_id, outcome, &mut out),
            Input::Timer(timer) => self.on_timer(timer, &mut out),
            Input::SetMic(on) => {
                self.mic = on;
                self.push_inputs(&mut out);
            }
            Input::SetCamera(on) => {
                self.camera = on;
                self.push_inputs(&mut out);
            }
            Input::Leave => self.leave(&mut out),
            Input::Abandon => {
                if self.is_waiting() {
                    self.stop_media(&mut out);
                    self.stage = Stage::Ended;
                }
            }
        }
        self.sync_subscriptions(&mut out);
        out
    }

    pub fn view(&self) -> CallView {
        let phase = match &self.stage {
            Stage::Ended => return CallView::idle(),
            Stage::Waiting(_) => CallPhase::Waiting,
            Stage::Joining => CallPhase::Joining,
            Stage::Joined => CallPhase::Joined,
            Stage::Failed(reason) => CallPhase::Failed {
                reason: reason.clone(),
            },
        };
        let tiles = match self.stage {
            Stage::Joining | Stage::Joined => self.tiles(),
            _ => Vec::new(),
        };
        let active_speaker = self
            .active_speaker
            .clone()
            .filter(|key| tiles.iter().any(|t| &t.key == key));
        CallView {
            phase,
            source: Some(self.source),
            room_name: self.zone.room_name.clone(),
            mic_on: self.mic,
            camera_on: self.camera,
            tiles,
            active_speaker,
        }
    }

    fn on_media(&mut self, attempt: u64, event: MediaEvent, out: &mut Vec<Effect>) {
        if attempt != self.attempt || !self.media_live {
            return;
        }
        match event {
            MediaEvent::Joined => {
                if self.stage == Stage::Joining {
                    self.stage = Stage::Joined;
                    self.retries = 0;
                }
            }
            MediaEvent::JoinFailed { code, message } => {
                let exhausted = if is_token_problem(&code) || is_token_problem(&message) {
                    CallFailure::TokenRetriesExhausted
                } else {
                    CallFailure::Media {
                        detail: if message.is_empty() { code } else { message },
                    }
                };
                self.stop_media(out);
                self.recover(exhausted, out);
            }
            MediaEvent::Left => {
                if self.stage == Stage::Joined {
                    self.stop_media(out);
                    self.recover(CallFailure::TokenRetriesExhausted, out);
                }
            }
            MediaEvent::Error { message } => {
                if is_token_problem(&message) {
                    self.stop_media(out);
                    self.recover(CallFailure::TokenRetriesExhausted, out);
                }
            }
            MediaEvent::Participants(participants) => {
                let ids: HashSet<&str> = participants.iter().map(|p| p.id.as_str()).collect();
                self.levels.retain(|id, _| ids.contains(id.as_str()));
                self.participants = participants;
            }
            MediaEvent::AudioLevels(levels) => self.on_levels(levels),
        }
    }

    fn on_levels(&mut self, levels: Vec<AudioLevel>) {
        let before = self.speaking_keys();
        for level in levels {
            self.levels.insert(level.participant_id, level.level);
        }
        let after = self.speaking_keys();
        if before == after {
            return;
        }
        let loudest = self
            .visible()
            .filter(|(p, _)| !p.is_local && self.is_speaking(p))
            .max_by(|(a, _), (b, _)| self.level(a).total_cmp(&self.level(b)))
            .map(|(_, key)| key);
        if let Some(key) = loudest {
            self.active_speaker = Some(key);
        }
    }

    fn on_credentials(&mut self, credentials: Option<Credentials>, out: &mut Vec<Effect>) {
        if self.source != CallSource::Office {
            return;
        }
        self.credentials = credentials.clone();
        match (&self.stage, credentials) {
            (Stage::Joining | Stage::Joined, None) => {
                self.stop_media(out);
                self.stage = Stage::Waiting(Wait::Credentials);
            }
            (Stage::Joining | Stage::Joined, Some(c))
                if self.joined.as_ref().map(|j| &j.url) != Some(&c.url) =>
            {
                self.stop_media(out);
                self.join(c, out);
            }
            (Stage::Waiting(Wait::Credentials), Some(c))
                if self.spent_token.as_ref() != Some(&c.token) =>
            {
                self.join(c, out);
            }
            _ => {}
        }
    }

    fn on_granted(&mut self, result: Result<Grant, HandoffFailure>, out: &mut Vec<Effect>) {
        let waiting = self.stage == Stage::Waiting(Wait::Grant);
        match result {
            Ok(grant) if !waiting => out.push(Effect::Reclaim {
                lease_id: grant.lease_id,
            }),
            Ok(grant) => {
                self.lease = Some(grant.lease_id.clone());
                if !grant.room_name.is_empty() {
                    self.zone.room_name = grant.room_name;
                }
                if self.zone.mates.is_empty() {
                    self.zone.mates = grant
                        .peers
                        .into_iter()
                        .map(|key| Mate {
                            key,
                            name: String::new(),
                            avatar: String::new(),
                            presence: Presence::Offline,
                        })
                        .collect();
                }
                out.push(Effect::Schedule {
                    after: RENEW_EVERY,
                    timer: Timer::Renew {
                        lease_id: grant.lease_id,
                    },
                });
                if !self.polling {
                    self.polling = true;
                    out.push(Effect::FetchDesktop);
                }
                self.join(grant.credentials, out);
            }
            Err(_) if !waiting => {}
            Err(HandoffFailure::NotInCall) => self.fail(CallFailure::DesktopNotInCall, out),
            Err(HandoffFailure::Unreachable) if self.retries == 0 => {
                self.fail(CallFailure::RelayUnreachable, out)
            }
            Err(HandoffFailure::Unreachable) => self.recover(CallFailure::RelayUnreachable, out),
        }
    }

    fn on_renewed(&mut self, lease_id: String, outcome: RenewOutcome, out: &mut Vec<Effect>) {
        if self.lease.as_ref() != Some(&lease_id) || !self.is_live() {
            return;
        }
        match outcome {
            RenewOutcome::Renewed | RenewOutcome::Unreachable => out.push(Effect::Schedule {
                after: RENEW_EVERY,
                timer: Timer::Renew { lease_id },
            }),
            RenewOutcome::Stale => {
                self.lease = None;
                self.fail(CallFailure::TakenOver, out);
            }
        }
    }

    fn on_polled(&mut self, zone: Option<CallZone>, out: &mut Vec<Effect>) {
        if !self.polling || !self.is_live() {
            return;
        }
        if let Some(zone) = zone {
            self.apply_zone(zone);
        }
        out.push(Effect::Schedule {
            after: POLL_EVERY,
            timer: Timer::Poll,
        });
    }

    fn on_timer(&mut self, timer: Timer, out: &mut Vec<Effect>) {
        match timer {
            Timer::Retry { generation }
                if generation == self.generation && self.stage == Stage::Waiting(Wait::Backoff) =>
            {
                match self.source {
                    CallSource::Office => {
                        let fresh = self
                            .credentials
                            .clone()
                            .filter(|c| self.spent_token.as_ref() != Some(&c.token));
                        match fresh {
                            Some(c) => self.join(c, out),
                            None => {
                                self.stage = Stage::Waiting(Wait::Credentials);
                                out.push(Effect::RefreshToken);
                            }
                        }
                    }
                    CallSource::Desktop => {
                        self.stage = Stage::Waiting(Wait::Grant);
                        out.push(Effect::Handoff);
                    }
                }
            }
            Timer::Renew { lease_id }
                if self.lease.as_ref() == Some(&lease_id) && self.is_live() =>
            {
                out.push(Effect::Renew { lease_id });
            }
            Timer::Poll if self.polling && self.is_live() => out.push(Effect::FetchDesktop),
            _ => {}
        }
    }

    fn leave(&mut self, out: &mut Vec<Effect>) {
        self.stop_media(out);
        match self.source {
            CallSource::Office => out.push(Effect::LeaveOffice),
            CallSource::Desktop => {
                if let Some(lease_id) = self.lease.take() {
                    out.push(Effect::Reclaim { lease_id });
                }
            }
        }
        self.polling = false;
        self.stage = Stage::Ended;
    }

    fn join(&mut self, credentials: Credentials, out: &mut Vec<Effect>) {
        self.attempt += 1;
        self.media_live = true;
        self.participants.clear();
        self.levels.clear();
        self.subscribed = None;
        self.stage = Stage::Joining;
        out.push(Effect::Join(MediaJoin {
            attempt: self.attempt,
            url: credentials.url.clone(),
            token: credentials.token.clone(),
            mic: self.mic,
            camera: self.camera,
        }));
        self.joined = Some(credentials);
    }

    fn stop_media(&mut self, out: &mut Vec<Effect>) {
        if self.media_live {
            self.media_live = false;
            out.push(Effect::Leave {
                attempt: self.attempt,
            });
        }
        self.participants.clear();
        self.levels.clear();
        self.subscribed = None;
    }

    fn recover(&mut self, exhausted: CallFailure, out: &mut Vec<Effect>) {
        if self.retries >= MAX_RETRIES {
            self.fail(exhausted, out);
            return;
        }
        self.retries += 1;
        self.generation += 1;
        if let Some(joined) = &self.joined {
            self.spent_token = Some(joined.token.clone());
        }
        self.stage = Stage::Waiting(Wait::Backoff);
        out.push(Effect::Schedule {
            after: Duration::from_secs(u64::from(self.retries)),
            timer: Timer::Retry {
                generation: self.generation,
            },
        });
    }

    fn fail(&mut self, failure: CallFailure, out: &mut Vec<Effect>) {
        self.stop_media(out);
        if self.source == CallSource::Desktop
            && failure != CallFailure::TakenOver
            && let Some(lease_id) = self.lease.take()
        {
            out.push(Effect::Reclaim { lease_id });
        }
        self.polling = false;
        self.stage = Stage::Failed(failure);
    }

    fn push_inputs(&self, out: &mut Vec<Effect>) {
        if self.media_live {
            out.push(Effect::SetInputs {
                attempt: self.attempt,
                mic: self.mic,
                camera: self.camera,
            });
        }
    }

    fn apply_zone(&mut self, zone: CallZone) {
        let room_name = if zone.room_name.is_empty() {
            std::mem::take(&mut self.zone.room_name)
        } else {
            zone.room_name
        };
        self.zone = CallZone {
            room_name,
            me: zone.me,
            mates: zone.mates,
        };
    }

    fn sync_subscriptions(&mut self, out: &mut Vec<Effect>) {
        if self.stage != Stage::Joined {
            return;
        }
        let mut wanted: Vec<String> = self
            .visible()
            .filter(|(p, _)| !p.is_local)
            .map(|(p, _)| p.id.clone())
            .collect();
        wanted.sort();
        if self.subscribed.as_ref() != Some(&wanted) {
            self.subscribed = Some(wanted.clone());
            out.push(Effect::Subscribe {
                attempt: self.attempt,
                participant_ids: wanted,
            });
        }
    }

    fn visible(&self) -> impl Iterator<Item = (&MediaParticipant, String)> {
        self.participants.iter().filter_map(|p| {
            let key = p.user_id.clone().unwrap_or_else(|| p.id.clone());
            (p.is_local || self.mate(&key).is_some()).then_some((p, key))
        })
    }

    fn mate(&self, key: &str) -> Option<&Mate> {
        self.zone.mates.iter().find(|m| m.key == key)
    }

    fn level(&self, p: &MediaParticipant) -> f32 {
        self.levels.get(&p.id).copied().unwrap_or_default()
    }

    fn is_speaking(&self, p: &MediaParticipant) -> bool {
        self.level(p) > SPEAKING_LEVEL && (!p.is_local || self.mic)
    }

    fn speaking_keys(&self) -> HashSet<String> {
        self.visible()
            .filter(|(p, _)| self.is_speaking(p))
            .map(|(_, key)| key)
            .collect()
    }

    fn tiles(&self) -> Vec<CallTile> {
        let mut seen = HashSet::new();
        let mut tiles: Vec<CallTile> = self
            .visible()
            .map(|(p, key)| {
                let person = if p.is_local {
                    self.zone.me.as_ref()
                } else {
                    self.mate(&key)
                }
                .filter(|m| !m.name.is_empty());
                seen.insert(key.clone());
                CallTile {
                    participant_id: Some(p.id.clone()),
                    name: person
                        .map(|m| m.name.clone())
                        .or_else(|| p.user_name.clone().filter(|n| !n.is_empty()))
                        .unwrap_or_else(|| GUEST.into()),
                    avatar: person.map(|m| m.avatar.clone()).unwrap_or_default(),
                    presence: person.map_or(Presence::Offline, |m| m.presence),
                    is_local: p.is_local,
                    in_call: true,
                    mic_on: p.mic_on,
                    has_camera: p.has_camera,
                    has_screen: p.has_screen,
                    speaking: self.is_speaking(p),
                    key,
                }
            })
            .collect();
        tiles.extend(
            self.zone
                .mates
                .iter()
                .filter(|m| !m.name.is_empty() && !seen.contains(&m.key))
                .map(|m| CallTile {
                    key: m.key.clone(),
                    participant_id: None,
                    name: m.name.clone(),
                    avatar: m.avatar.clone(),
                    presence: m.presence,
                    is_local: false,
                    in_call: false,
                    mic_on: false,
                    has_camera: false,
                    has_screen: false,
                    speaking: false,
                }),
        );
        tiles.sort_by(|a, b| {
            (!a.is_local, !a.in_call, &a.name, &a.key).cmp(&(
                !b.is_local,
                !b.in_call,
                &b.name,
                &b.key,
            ))
        });
        tiles
    }
}
