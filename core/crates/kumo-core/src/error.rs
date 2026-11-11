use crate::api::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("auth failed: {code}")]
    Auth { code: String },
    #[error("not signed in")]
    NotSignedIn,
    #[error("network: {0}")]
    Network(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("socket: {0}")]
    Socket(String),
    #[error("storage: {0}")]
    Storage(String),
    #[error("sendbird {code}: {message}")]
    Sendbird { code: i64, message: String },
    #[error("timeout")]
    Timeout,
}

impl From<reqwest::Error> for CoreError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_decode() {
            CoreError::Decode(e.to_string())
        } else {
            CoreError::Network(e.to_string())
        }
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        CoreError::Decode(e.to_string())
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for CoreError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        CoreError::Socket(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
