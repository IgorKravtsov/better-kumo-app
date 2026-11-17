use crate::desktop::Presence;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallSource {
    Office,
    Desktop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallFailure {
    TakenOver,
    TokenRetriesExhausted,
    DesktopNotInCall,
    RelayUnreachable,
    Media { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallPhase {
    Idle,
    Waiting,
    Joining,
    Joined,
    Failed { reason: CallFailure },
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallView {
    pub phase: CallPhase,
    pub source: Option<CallSource>,
    pub room_name: String,
    pub mic_on: bool,
    pub camera_on: bool,
    pub tiles: Vec<CallTile>,
    pub active_speaker: Option<String>,
}

impl CallView {
    pub fn idle() -> Self {
        Self {
            phase: CallPhase::Idle,
            source: None,
            room_name: String::new(),
            mic_on: false,
            camera_on: false,
            tiles: Vec::new(),
            active_speaker: None,
        }
    }
}

impl Default for CallView {
    fn default() -> Self {
        Self::idle()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mate {
    pub key: String,
    pub name: String,
    pub avatar: String,
    pub presence: Presence,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallZone {
    pub room_name: String,
    pub me: Option<Mate>,
    pub mates: Vec<Mate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub url: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub lease_id: String,
    pub credentials: Credentials,
    pub room_name: String,
    pub peers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffFailure {
    NotInCall,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenewOutcome {
    Renewed,
    Stale,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaParticipant {
    pub id: String,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub is_local: bool,
    pub mic_on: bool,
    pub has_camera: bool,
    pub has_screen: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioLevel {
    pub participant_id: String,
    pub level: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MediaEvent {
    Joined,
    JoinFailed { code: String, message: String },
    Left,
    Error { message: String },
    Participants(Vec<MediaParticipant>),
    AudioLevels(Vec<AudioLevel>),
}

pub fn is_token_problem(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("exp-token")
        || m.contains("unauthorized")
        || (m.contains("token") && m.contains("invalid"))
}
