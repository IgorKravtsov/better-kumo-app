use std::time::Duration;

use anyhow::bail;
use kumo_core::desktop::routes;
use kumo_core::events::EventsPage;

use crate::kumo::relay_setting;

const DEFAULT_LISTEN: &str = "0.0.0.0:8787";
const SLACK: Duration = Duration::from_secs(10);

pub struct RelayTarget {
    url: String,
    secret: String,
}

impl RelayTarget {
    pub fn from_config() -> anyhow::Result<Self> {
        let Some(secret) = relay_setting("secret") else {
            bail!(
                "office events come from Better Kumo, and it is not set up on this computer (no ~/.config/kumowatch/relay.toml)"
            );
        };
        let listen = relay_setting("listen").unwrap_or_else(|| DEFAULT_LISTEN.into());
        let port = listen.rsplit(':').next().unwrap_or("8787");
        Ok(Self {
            url: format!("http://127.0.0.1:{port}{}", routes::EVENTS),
            secret,
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
/// all of them were office events.
pub fn missed(cursor: Option<&(String, u64)>, page: &EventsPage) -> u64 {
    match (cursor, page.events.first()) {
        (Some((boot, after)), Some(first)) => {
            let expected = if *boot == page.boot { after + 1 } else { 1 };
            first.seq.saturating_sub(expected)
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
