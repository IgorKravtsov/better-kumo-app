use std::time::Duration;

use anyhow::bail;
use kumo_core::desktop::routes;
use kumo_core::events::EventsPage;

use crate::kumo::{relay_config, relay_setting};

const SLACK: Duration = Duration::from_secs(10);

pub struct RelayTarget {
    url: String,
    secret: String,
}

impl RelayTarget {
    pub fn from_config() -> anyhow::Result<Self> {
        match relay_config() {
            Ok(Some(config)) => Self::from_table(&config),
            Ok(None) => bail!(
                "office events come from Better Kumo, and it is not set up on this computer (no ~/.config/kumowatch/relay.toml)"
            ),
            Err(problem) => bail!("{problem}; fix ~/.config/kumowatch/relay.toml in place"),
        }
    }

    fn from_table(config: &toml::Table) -> anyhow::Result<Self> {
        let Some(secret) = relay_setting(config, "secret").filter(|s| !s.trim().is_empty()) else {
            bail!(
                "office events come from Better Kumo, and it is not set up on this computer (relay.toml has an empty secret; set a long random `secret` in it)"
            );
        };
        if reqwest::header::HeaderValue::from_str(&format!("Bearer {secret}")).is_err() {
            bail!(
                "relay.toml has a `secret` that can't be sent in a header (a line break or control character); set a long random `secret` in it"
            );
        }
        let Some(port) = relay_setting(config, "listen").and_then(listen_port) else {
            bail!("relay.toml has no valid `listen` address; fix it in place");
        };
        Ok(Self {
            url: format!("http://127.0.0.1:{port}{}", routes::EVENTS),
            secret: secret.to_string(),
        })
    }

    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        cursor: Option<&(String, u64)>,
        wait: Duration,
    ) -> anyhow::Result<EventsPage> {
        let mut query = vec![("wait", wait.as_secs().to_string())];
        if let Some((boot, after)) = cursor {
            query.push(("boot", boot.clone()));
            query.push(("after", after.to_string()));
        }
        let response = http
            .get(&self.url)
            .bearer_auth(&self.secret)
            .query(&query)
            .timeout(wait + SLACK)
            .send()
            .await;
        let response = match response {
            Ok(r) => r,
            Err(e) if e.is_connect() => {
                bail!("office events come from Better Kumo, and it is not running on this computer")
            }
            Err(e) => bail!("could not ask Better Kumo for office events: {e}"),
        };
        match response.status().as_u16() {
            200 => Ok(response.json().await?),
            404 => bail!("Better Kumo on this computer is too old for office events; update it"),
            401 => bail!("Better Kumo refused the secret from relay.toml"),
            status => bail!("Better Kumo answered {status} for office events"),
        }
    }
}

/// The port of a `host:port` the relay could bind; anything else must not receive the secret.
fn listen_port(listen: &str) -> Option<u16> {
    if let Ok(addr) = listen.parse::<std::net::SocketAddr>() {
        return Some(addr.port());
    }
    let (host, port) = listen.rsplit_once(':')?;
    let hostname = !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    hostname.then(|| port.parse().ok()).flatten()
}

pub fn next_cursor(cursor: Option<&(String, u64)>, page: &EventsPage) -> (String, u64) {
    let same_boot = cursor.is_some_and(|(boot, _)| *boot == page.boot);
    let after = match (cursor, page.events.last()) {
        (_, Some(last)) => last.seq,
        (None, None) => page.latest,
        (Some((_, after)), None) if same_boot => *after,
        (Some(_), None) => 0,
    };
    (page.boot.clone(), after)
}

/// Log entries the Relay evicted before `cursor` reached them; chat messages share the log, so not
/// all of them were office events. Office and chat are evicted separately, so gaps can sit between
/// returned entries too.
pub fn missed(cursor: Option<&(String, u64)>, page: &EventsPage) -> u64 {
    match (cursor, page.events.last()) {
        (Some((boot, after)), Some(last)) => {
            let after = if *boot == page.boot { *after } else { 0 };
            last.seq
                .saturating_sub(after)
                .saturating_sub(page.events.len() as u64)
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kumo::parse_relay_config;

    fn target(text: &str) -> anyhow::Result<RelayTarget> {
        RelayTarget::from_table(&parse_relay_config(text).map_err(anyhow::Error::msg)?)
    }

    fn error(text: &str) -> String {
        target(text).err().unwrap().to_string()
    }

    #[test]
    fn a_blank_secret_is_not_set_up() {
        for secret in [
            r#""""#,
            r#""   ""#,
            "'  '",
            r#""\t""#,
            r#""\u0020""#,
            "'''\n'''",
        ] {
            let text = format!("listen = \"0.0.0.0:8787\"\nsecret = {secret}\n");
            let error = error(&text);
            assert!(error.contains("empty secret"), "{secret}: {error}");
        }
        assert!(error("listen = \"0.0.0.0:8787\"\n").contains("empty secret"));
    }

    #[test]
    fn a_set_secret_is_decoded() {
        let quoted = target("listen = '0.0.0.0:9000'\nsecret = 'a\"b'\n").unwrap();
        assert_eq!(quoted.secret, "a\"b");
        assert_eq!(
            quoted.url,
            format!("http://127.0.0.1:9000{}", routes::EVENTS)
        );
        let escaped = target("listen = \"[::]:8787\"\nsecret = \"x\\u0079\"\n").unwrap();
        assert_eq!(escaped.secret, "xy");
        assert_eq!(
            escaped.url,
            format!("http://127.0.0.1:8787{}", routes::EVENTS)
        );
    }

    #[test]
    fn an_invalid_listen_address_sends_no_secret() {
        for listen in [
            "",
            "listen = true\n",
            "listen = '   '\n",
            "listen = '0.0.0.0:99999'\n",
            "listen = '0.0.0.0:80/x'\n",
            "listen = 'http://127.0.0.1:8787'\n",
            "listen = 'bad host:8787'\n",
        ] {
            let error = error(&format!("{listen}secret = 's3cr3t'\n"));
            assert!(error.contains("no valid `listen`"), "{listen}: {error}");
        }
        assert_eq!(listen_port("localhost:8787"), Some(8787));
        assert_eq!(listen_port("[::1]:9000"), Some(9000));
    }

    #[test]
    fn a_secret_that_cannot_be_a_header_is_refused() {
        let error = error("listen = 'localhost:8787'\nsecret = \"zq\\nzq\"\n");
        assert!(error.contains("can't be sent in a header"), "{error}");
        assert!(!error.contains("zq"), "{error}");
    }

    #[test]
    fn invalid_toml_is_not_quoted() {
        let error = error("listen = '0.0.0.0:8787'\nsecret = \"s3cr3t-token\n");
        assert!(
            error.starts_with("relay.toml is not valid TOML at line 2"),
            "{error}"
        );
        assert!(!error.contains("s3cr3t"), "{error}");
    }
    use kumo_core::events::{EventKind, LoggedEvent, OfficeEvent, Who};

    fn page(boot: &str, latest: u64, seqs: &[u64]) -> EventsPage {
        EventsPage {
            boot: boot.into(),
            latest,
            events: seqs
                .iter()
                .map(|seq| LoggedEvent {
                    seq: *seq,
                    event: OfficeEvent::new(EventKind::Nudge, 0, Who::default()),
                })
                .collect(),
            desktop_connected: true,
        }
    }

    #[test]
    fn the_cursor_never_skips_an_event() {
        assert_eq!(next_cursor(None, &page("b", 7, &[])), ("b".into(), 7));
        let at_seven = ("b".to_string(), 7);
        assert_eq!(
            next_cursor(Some(&at_seven), &page("b", 10, &[8, 9])),
            ("b".into(), 9)
        );
        assert_eq!(
            next_cursor(Some(&at_seven), &page("b", 9, &[])),
            ("b".into(), 7)
        );
        assert_eq!(
            next_cursor(Some(&at_seven), &page("new", 2, &[])),
            ("new".into(), 0)
        );
    }

    #[test]
    fn counts_events_evicted_before_the_cursor() {
        let at_seven = ("b".to_string(), 7);
        assert_eq!(missed(None, &page("b", 20, &[15, 16])), 0);
        assert_eq!(missed(Some(&at_seven), &page("b", 9, &[8, 9])), 0);
        assert_eq!(missed(Some(&at_seven), &page("b", 20, &[12, 13])), 4);
        assert_eq!(missed(Some(&at_seven), &page("b", 7, &[])), 0);
        assert_eq!(missed(Some(&at_seven), &page("new", 3, &[1, 2, 3])), 0);
        assert_eq!(missed(Some(&at_seven), &page("new", 9, &[4, 5])), 3);
        // Chat seq 8 kept while office seq 9 was evicted.
        assert_eq!(missed(Some(&at_seven), &page("b", 12, &[8, 10, 12])), 2);
    }
}
