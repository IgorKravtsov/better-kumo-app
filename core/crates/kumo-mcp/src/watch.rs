use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;
use serde_json::{Value, json};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::kumo::{ACCESS_OFF, Connection};
use crate::server::{KumoServer, RuleParams, ServerOptions, rules_from};

const OFFICE_PIN: Duration = Duration::from_secs(2);
// A reader that stopped reading must not keep the process alive after it should end.
const LAST_LINE: Duration = Duration::from_secs(1);

pub struct WatchArgs {
    pub people: Option<Vec<String>>,
    pub kinds: Option<Vec<String>>,
    pub places: Option<Vec<String>>,
    pub label: Option<String>,
    pub rules: Option<String>,
}

impl WatchArgs {
    fn rules(self) -> anyhow::Result<Option<kumo_core::events::WatchRules>> {
        if let Some(json) = self.rules {
            let params: Vec<RuleParams> = serde_json::from_str(&json)
                .map_err(|e| anyhow::anyhow!("--rules is not a JSON array of rules: {e}"))?;
            return rules_from(&params).map(Some);
        }
        if self.people.is_none()
            && self.kinds.is_none()
            && self.places.is_none()
            && self.label.is_none()
        {
            return Ok(None);
        }
        rules_from(&[RuleParams {
            people: self.people,
            kinds: self.kinds,
            places: self.places,
            label: self.label,
        }])
        .map(Some)
    }
}

async fn write(out: &mut (impl AsyncWrite + Unpin), value: &Value) -> anyhow::Result<()> {
    out.write_all(format!("{value}\n").as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

pub async fn watch(connection: Arc<Connection>, args: WatchArgs) -> anyhow::Result<()> {
    let mut out = tokio::io::stdout();
    let result = run(&connection, args, &mut out).await;
    connection.close().await;
    // Returning would drop the runtime, which waits for a stdout write a stalled reader never takes.
    let Err(e) = result else {
        std::process::exit(0)
    };
    let line = json!({ "kind": "error", "text": format!("{e:#}") });
    let _ = tokio::time::timeout(LAST_LINE, write(&mut out, &line)).await;
    std::process::exit(1);
}

async fn run(
    connection: &Arc<Connection>,
    args: WatchArgs,
    out: &mut (impl AsyncWrite + Unpin),
) -> anyhow::Result<()> {
    let given = args.rules()?;
    // The cursors are taken before connecting: messages and office events that arrive while
    // chat syncs are news, and the sync itself does not report history as arrivals.
    let server = KumoServer::new(connection.clone(), ServerOptions::default());
    let wants_office = server.event_rules(given.as_ref())?.wants_office();
    let revoked = server.access_revoked();
    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(revoked, interrupted);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Value>(32);
    if wants_office {
        let failure = match tokio::time::timeout(OFFICE_PIN, server.start_office_cursor()).await {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(format!("{e:#}")),
            Err(_) => Some("Better Kumo did not answer in time".to_string()),
        };
        if let Some(why) = failure {
            let note = json!({
                "kind": "note",
                "text": format!("Office events are counted from the first successful check, not from now: {why}"),
            });
            let _ = tx.try_send(note);
        }
    }
    tokio::select! {
        biased;
        _ = &mut revoked => bail!(ACCESS_OFF),
        _ = &mut interrupted => return Ok(()),
        ready = connection.ready() => { ready?; }
    }
    eprintln!("kumo-mcp: watching; one JSON object per line on stdout, Ctrl-C to stop");
    let producer = {
        let server = server.clone();
        async move { server.stream_events(given, &tx).await }
    };
    tokio::pin!(producer);
    let mut failed: Option<anyhow::Error> = None;
    loop {
        let value = tokio::select! {
            biased;
            // Queued events were collected under access that has since been withdrawn.
            _ = &mut revoked => bail!(ACCESS_OFF),
            _ = &mut interrupted => return Ok(()),
            result = &mut producer, if failed.is_none() => {
                let e = result.err().unwrap_or_else(|| anyhow::anyhow!("the event stream ended"));
                if !server.access_allowed() {
                    bail!(ACCESS_OFF);
                }
                failed = Some(e);
                continue;
            }
            value = rx.recv() => match value {
                Some(value) => value,
                None => break,
            },
        };
        tokio::select! {
            biased;
            _ = &mut revoked => bail!(ACCESS_OFF),
            _ = &mut interrupted => return Ok(()),
            written = write(out, &value) => written?,
        }
    }
    Err(failed.unwrap_or_else(|| anyhow::anyhow!("the event stream ended")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kumo::ConnectOptions;

    #[tokio::test]
    async fn revoking_access_ends_a_watch_whose_reader_stopped_reading() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents.json");
        let connection = Arc::new(Connection::for_test(ConnectOptions {
            session_path: dir.path().join("session.json"),
            space: None,
            db_path: Some(dir.path().join("chat.db")),
            devtools_port: 1,
            agents_path: agents.clone(),
            watch_path: dir.path().join("watch.json"),
            cache_tag: "",
        }));
        let (mut out, _stalled_reader) = tokio::io::duplex(64);
        let args = WatchArgs {
            people: None,
            kinds: Some(vec!["message".into()]),
            places: None,
            label: None,
            rules: None,
        };
        let watching = {
            let connection = connection.clone();
            tokio::spawn(async move { run(&connection, args, &mut out).await })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        for id in 0..50 {
            connection
                .arrivals()
                .push(kumo_core::chat::IncomingMessage {
                    channel_url: "chat".into(),
                    channel_title: "Team".into(),
                    category: kumo_core::chat::ChannelCategory::Direct,
                    message_id: id,
                    parent_id: None,
                    created_at: 0,
                    sender_id: "amy".into(),
                    sender_name: "amy".into(),
                    text: "hi".into(),
                    mentions_me: false,
                    channel_is_open: false,
                });
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::fs::write(&agents, r#"{"enabled":false}"#).unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), watching)
            .await
            .expect("a stalled reader kept the watch alive after access was turned off")
            .unwrap();
        assert_eq!(ended.unwrap_err().to_string(), ACCESS_OFF);
    }
}
