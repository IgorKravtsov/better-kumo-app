use serde::Deserialize;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    RecentSpaces,
    DefaultSpace,
    SelfSpaces,
    Space {
        space_name: String,
    },
    SpaceChatConfig {
        space_name: String,
    },
    SpaceUserRoles {
        space_name: String,
        next: Option<String>,
    },
    Recording {
        recording_id: String,
    },
    CreateChatChannel,
    SoftDeleteFileMessage {
        channel_url: String,
        message_id: String,
    },
    ResetPassword,
    DesktopLoginToken,
    DeleteChatMessage {
        channel_url: String,
        message_id: String,
    },
}

impl Endpoint {
    pub fn method(&self) -> HttpMethod {
        match self {
            Endpoint::CreateChatChannel | Endpoint::ResetPassword | Endpoint::DesktopLoginToken => {
                HttpMethod::Post
            }
            Endpoint::SoftDeleteFileMessage { .. } | Endpoint::DeleteChatMessage { .. } => {
                HttpMethod::Delete
            }
            _ => HttpMethod::Get,
        }
    }

    pub fn segments(&self) -> Vec<String> {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        match self {
            Endpoint::RecentSpaces => s(&["users", "me", "recentSpaces"]),
            Endpoint::DefaultSpace => s(&["users", "me", "defaultSpace"]),
            Endpoint::SelfSpaces => s(&["spaces"]),
            Endpoint::Space { space_name } => s(&["spaces", space_name]),
            Endpoint::SpaceChatConfig { space_name } => s(&["spaces", space_name, "chatConfig"]),
            Endpoint::SpaceUserRoles { space_name, .. } => s(&["spaces", space_name, "userRoles"]),
            Endpoint::Recording { recording_id } => s(&["recordings", recording_id]),
            Endpoint::CreateChatChannel => s(&["chat", "channel"]),
            Endpoint::SoftDeleteFileMessage {
                channel_url,
                message_id,
            } => s(&["chat", "channel", channel_url, "file-messages", message_id]),
            Endpoint::ResetPassword => s(&["account", "resetPassword"]),
            Endpoint::DesktopLoginToken => s(&["account", "desktop", "token"]),
            Endpoint::DeleteChatMessage {
                channel_url,
                message_id,
            } => s(&["chat", "channel", channel_url, "messages", message_id]),
        }
    }

    pub fn query(&self) -> Vec<(String, String)> {
        let q = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(k, val)| (k.to_string(), val.to_string()))
                .collect::<Vec<_>>()
        };
        match self {
            Endpoint::RecentSpaces => q(&[("excludeDemo", "true")]),
            Endpoint::SelfSpaces => q(&[("members", "true"), ("excludeDemo", "true")]),
            Endpoint::SpaceUserRoles { next, .. } => {
                let mut params = q(&[("includeSpaceMetadata", "true")]);
                if let Some(cursor) = next {
                    params.push(("next".into(), cursor.clone()));
                }
                params
            }
            _ => Vec::new(),
        }
    }

    pub fn url(&self, api_base_url: &str) -> Result<Url, url::ParseError> {
        let mut url = Url::parse(api_base_url)?;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| url::ParseError::RelativeUrlWithCannotBeABaseBase)?;
            path.clear().push("v1");
            for segment in self.segments() {
                path.push(&segment);
            }
        }
        let query = self.query();
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaginationMetadata {
    pub count: u64,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Paginated<T> {
    pub items: Vec<T>,
    pub metadata: PaginationMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorReason {
    Unauthorized,
    Forbidden,
    BadRequest,
    InternalServerError,
    NetworkFailure,
    Unknown,
}

impl ApiErrorReason {
    pub fn from_status(status: u16) -> Self {
        match status {
            400 => Self::BadRequest,
            401 => Self::Unauthorized,
            403 => Self::Forbidden,
            500 => Self::InternalServerError,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{title}: {detail}")]
pub struct ApiError {
    pub title: String,
    pub detail: String,
    pub reason: ApiErrorReason,
    pub status: i32,
}

#[derive(Deserialize)]
struct ErrorBody {
    title: Option<String>,
    detail: Option<String>,
}

impl ApiError {
    pub fn from_response(status: u16, body: &str) -> Self {
        let parsed = serde_json::from_str::<ErrorBody>(body).ok();
        let (title, detail) = parsed.map(|b| (b.title, b.detail)).unwrap_or((None, None));
        Self {
            title: title.unwrap_or_else(|| "Non-successful response".into()),
            detail: detail.unwrap_or_else(|| "Response was not successful".into()),
            reason: ApiErrorReason::from_status(status),
            status: status.into(),
        }
    }

    pub fn network_failure() -> Self {
        Self {
            title: "Network failure".into(),
            detail: "Could not connect; please try again later.".into(),
            reason: ApiErrorReason::NetworkFailure,
            status: -1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://api.kumospace.com";

    #[test]
    fn builds_paths_like_the_official_client() {
        assert_eq!(
            Endpoint::RecentSpaces.url(BASE).unwrap().as_str(),
            "https://api.kumospace.com/v1/users/me/recentSpaces?excludeDemo=true"
        );
        assert_eq!(
            Endpoint::SpaceChatConfig {
                space_name: "gobabygo".into()
            }
            .url(BASE)
            .unwrap()
            .as_str(),
            "https://api.kumospace.com/v1/spaces/gobabygo/chatConfig"
        );
    }

    #[test]
    fn paginated_user_roles_carry_cursor() {
        let url = Endpoint::SpaceUserRoles {
            space_name: "gobabygo".into(),
            next: Some("abc".into()),
        }
        .url(BASE)
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://api.kumospace.com/v1/spaces/gobabygo/userRoles?includeSpaceMetadata=true&next=abc"
        );
    }

    #[test]
    fn channel_urls_are_escaped_as_one_segment() {
        let url = Endpoint::SoftDeleteFileMessage {
            channel_url: "space_a/b".into(),
            message_id: "42".into(),
        }
        .url(BASE)
        .unwrap();
        assert_eq!(url.path(), "/v1/chat/channel/space_a%2Fb/file-messages/42");
        assert_eq!(
            Endpoint::SoftDeleteFileMessage {
                channel_url: "x".into(),
                message_id: "1".into()
            }
            .method(),
            HttpMethod::Delete
        );
    }

    #[test]
    fn parses_pagination_and_errors() {
        let page: Paginated<serde_json::Value> =
            serde_json::from_str(r#"{"items":[1,2],"metadata":{"count":2,"next":null}}"#).unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.metadata.next, None);

        let err = ApiError::from_response(403, r#"{"title":"Nope","detail":"No access"}"#);
        assert_eq!(err.reason, ApiErrorReason::Forbidden);
        assert_eq!(err.title, "Nope");
        let fallback = ApiError::from_response(502, "<html>");
        assert_eq!(fallback.title, "Non-successful response");
        assert_eq!(fallback.reason, ApiErrorReason::Unknown);
    }
}
