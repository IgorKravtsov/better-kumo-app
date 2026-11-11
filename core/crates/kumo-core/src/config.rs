#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KumoConfig {
    pub api_base_url: String,
    pub ws_base_url: String,
    pub sendbird_app_id: String,
    pub firebase_api_key: String,
}

impl Default for KumoConfig {
    fn default() -> Self {
        Self {
            api_base_url: "https://api.kumospace.com".into(),
            ws_base_url: "wss://ws.kumospace.com".into(),
            sendbird_app_id: "CA71E01B-3C4B-4311-A953-B3904C6D7AD9".into(),
            firebase_api_key: "AIzaSyDMr7rTdxo97qyyVQcD5iM0VVDD8yJ6mfY".into(),
        }
    }
}

impl KumoConfig {
    pub fn sendbird_api_base_url(&self) -> String {
        format!(
            "https://api-{}.sendbird.com",
            self.sendbird_app_id.to_lowercase()
        )
    }

    pub fn sendbird_ws_base_url(&self) -> String {
        format!(
            "wss://ws-{}.sendbird.com",
            self.sendbird_app_id.to_lowercase()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sendbird_hosts_use_lowercase_app_id() {
        let config = KumoConfig::default();
        assert_eq!(
            config.sendbird_ws_base_url(),
            "wss://ws-ca71e01b-3c4b-4311-a953-b3904c6d7ad9.sendbird.com"
        );
    }
}
