use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{CoreError, Result};

pub type HttpClient = reqwest::Client;

pub const USER_AGENT: &str = concat!("kumowatch/", env!("CARGO_PKG_VERSION"));

pub fn build_http_client() -> Result<reqwest::Client> {
    let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .filter_map(|der| reqwest::Certificate::from_der(der.as_ref()).ok());
    reqwest::Client::builder()
        .tls_certs_only(roots)
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| CoreError::Network(e.to_string()))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}
