mod agents;
mod api;
mod client;
mod framing;
mod nearby;
mod page;
mod state;

pub use crate::chat::notifications::{
    KindsRequest, MyStatus, NotificationKinds, NotificationProfile, NotificationsRequest,
    StatusSignals,
};
pub use agents::{AGENT_ACCESS_FILE, AgentAccess};
pub(crate) use agents::{config_path, save_atomically};
pub use api::{
    AgentsRequest, DEFAULT_NTFY_SERVER, DeviceRequest, FloorRequest, FollowRequest, Handoff,
    JoinPersonRequest, LeaseRequest, MediaRequest, MirroringRequest, MoveRequest, NtfyRequest,
    NtfyTarget, NudgeRequest, OkReply, RELAY_API, RelayCompatibility, RelayError, RelayStatus,
    StatusRequest, ViewingRequest, ZoneRequest, routes,
};
pub use client::{RelayClient, RelayPairing};
pub use framing::Rect;
pub use nearby::{NearbyChat, NearbyChatRequest, NearbyMessage, NearbyRoom};
pub use page::PageError;
pub use state::{
    DesktopCall, DesktopFloor, DesktopPerson, DesktopSelf, DesktopState, DesktopTarget,
    DesktopZone, Presence,
};
