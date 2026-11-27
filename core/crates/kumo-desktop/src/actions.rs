//! Settings and links are fixed lists, so a page can never make the app open an arbitrary address
//! or file.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Ready,
    Drag,
    Zoom,
    Finish,
    Open {
        link: Link,
    },
    OpenKumospace,
    RestartKumospace,
    InstallMeetings,
    ShowPairing,
    Set {
        setting: Setting,
        on: bool,
    },
    OpenSettings {
        #[serde(default)]
        section: Option<Section>,
    },
    OpenGuide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    KumospaceDownload,
    IphoneGuide,
    AndroidGuide,
    Tailscale,
    Log,
    MeetingsFolder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Setting {
    Agents,
    StartAtLogin,
    KeepKumospaceOpen,
    AutoUpdate,
    MeetingsAutoStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    General,
    Phones,
    Meetings,
}

impl Section {
    pub fn anchor(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Phones => "phones",
            Self::Meetings => "meetings",
        }
    }
}

const DOCS: &str = "https://github.com/IgorKravtsov/better-kumo-app/blob/main/docs";

impl Link {
    pub fn url(self) -> Option<String> {
        match self {
            Self::KumospaceDownload => Some("https://www.kumospace.com/download".into()),
            Self::IphoneGuide => Some(format!("{DOCS}/install-iphone.md")),
            Self::AndroidGuide => Some(format!("{DOCS}/install-android.md")),
            Self::Tailscale => Some("https://tailscale.com/download".into()),
            Self::Log | Self::MeetingsFolder => None,
        }
    }
}

impl Action {
    pub fn parse(message: &str) -> Option<Self> {
        serde_json::from_str(message).ok()
    }
}

pub fn may_open_in_browser(url: &str) -> bool {
    url.starts_with("https://github.com/IgorKravtsov/better-kumo-app/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_send_these_messages() {
        assert_eq!(Action::parse(r#"{"action":"ready"}"#), Some(Action::Ready));
        assert_eq!(
            Action::parse(r#"{"action":"set","setting":"start_at_login","on":true}"#),
            Some(Action::Set {
                setting: Setting::StartAtLogin,
                on: true
            })
        );
        assert_eq!(
            Action::parse(r#"{"action":"open_settings","section":"phones"}"#),
            Some(Action::OpenSettings {
                section: Some(Section::Phones)
            })
        );
        assert_eq!(
            Action::parse(r#"{"action":"open_settings"}"#),
            Some(Action::OpenSettings { section: None })
        );
    }

    #[test]
    fn anything_else_is_ignored() {
        for message in [
            "",
            "drag",
            r#"{"action":"open","link":"https://evil.example"}"#,
            r#"{"action":"open","url":"file:///etc/passwd"}"#,
            r#"{"action":"set","setting":"secret","on":true}"#,
            r#"{"action":"run","command":"rm -rf ~"}"#,
        ] {
            assert_eq!(Action::parse(message), None, "{message}");
        }
    }

    #[test]
    fn links_open_only_known_addresses() {
        assert_eq!(
            Link::IphoneGuide.url().as_deref(),
            Some(
                "https://github.com/IgorKravtsov/better-kumo-app/blob/main/docs/install-iphone.md"
            )
        );
        assert_eq!(Link::Log.url(), None);
        assert!(may_open_in_browser(&Link::AndroidGuide.url().unwrap()));
        assert!(!may_open_in_browser(
            "https://github.com/someone-else/phish"
        ));
        assert!(!may_open_in_browser("http://127.0.0.1:8787/settings"));
    }
}
