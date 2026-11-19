pub mod api;
pub mod app_session;
pub mod auth;
pub mod call;
pub mod channel;
pub mod chat;
pub mod client;
pub mod config;
pub mod desktop;
pub mod error;
pub mod events;
pub mod http;
pub mod sendbird;
pub mod session_file;
pub mod space;
pub mod ws;

pub use api::{ApiError, ApiErrorReason, Endpoint, HttpMethod, Paginated, PaginationMetadata};
pub use auth::{
    Auth, AuthSession, FirebaseAuth, MemorySessionStore, PasswordCredentials, SessionStore,
};
pub use channel::{ChannelKind, ChannelUrl};
pub use client::{ChatConfig, ChatConfigChannel, KumoApi};
pub use config::KumoConfig;
pub use error::{CoreError, Result};
pub use session_file::FileSessionStore;
pub use space::{SpaceConnection, SpaceEvent};
pub use ws::{ClientOp, ConnectParams, Platform, ServerOp, TeleportTarget};

pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
