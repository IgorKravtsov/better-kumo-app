use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use kumo_meetings::{
    MeetingPage, Meetings, MeetingsConfig, MeetingsDeps, Module, ProcessTranscriber,
};
use kumo_relay::{Config, Relay, RelayParts};
use serde_json::{Value, json};

struct Quiet;

impl MeetingPage for Quiet {
    fn evaluate<'a>(&'a self, _: &'a str) -> BoxFuture<'a, anyhow::Result<Value>> {
        Box::pin(async { Ok(json!({ "inCall": false, "me": null })) })
    }

    fn announce<'a>(&'a self, _: &'a str) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

struct Page {
    base: String,
    port: u16,
    http: reqwest::Client,
    dir: tempfile::TempDir,
    relay: Arc<Relay>,
}

impl Page {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let transcriber = dir.path().join("better-kumo-transcriber");
        std::fs::write(&transcriber, b"").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut config = Config::template("secret-0123456789abcdef", "office");
        config.listen = format!("127.0.0.1:{port}");
        config.data_dir = Some(dir.path().to_path_buf());
        config.meetings.transcriber = Some(transcriber.clone());
        config.meetings.folder = Some(dir.path().join("Meetings"));
        let config_path = dir.path().join("relay.toml");
        config.save(&config_path).unwrap();
        let meetings = Meetings::new(MeetingsDeps {
            page: Arc::new(Quiet),
            config: config.meetings.clone(),
            models_dir: dir.path().join("models"),
            module: Module::new(dir.path().join("module"), Some(transcriber)),
            transcriber: Arc::new(ProcessTranscriber),
        });
        let relay = Relay::new(RelayParts {
            uid: "uid-me".into(),
            data_dir: dir.path().to_path_buf(),
            apns: None,
            ntfy: None,
            config_path: Some(config_path),
            desktop: None,
            meetings: Some(meetings),
        });
        tokio::spawn(kumo_relay::serve(
            listener,
            relay.clone(),
            config,
            std::future::pending(),
        ));
        Self {
            base: format!("http://127.0.0.1:{port}"),
            port,
            http: reqwest::Client::new(),
            dir,
            relay,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .header("x-better-kumo", "settings")
    }

    fn saved(&self) -> Config {
        Config::load(&self.dir.path().join("relay.toml")).unwrap()
    }
}

#[tokio::test]
async fn the_page_opens_only_on_this_computer() {
    let page = Page::start().await;
    let html = page
        .http
        .get(format!("{}/settings", page.base))
        .send()
        .await
        .unwrap();
    assert_eq!(html.status(), 200);
    assert!(html.text().await.unwrap().contains("Better Kumo settings"));

    let rebound = page
        .http
        .get(format!("{}/settings", page.base))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rebound.status(), 403);
}

#[tokio::test]
async fn other_websites_cannot_read_or_change_settings() {
    let page = Page::start().await;
    let bare = page
        .http
        .get(format!("{}/settings/meetings", page.base))
        .send()
        .await
        .unwrap();
    assert_eq!(bare.status(), 403);

    let forged = page
        .request(reqwest::Method::POST, "/settings/meetings/module")
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 403);

    let own = page
        .request(reqwest::Method::GET, "/settings/meetings")
        .header("origin", format!("http://127.0.0.1:{}", page.port))
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 200);
    let body: Value = own.json().await.unwrap();
    assert_eq!(body["available"], true);
    assert_eq!(body["status"]["module"]["state"], "ready");
    assert_eq!(body["config"]["transcriber"], Value::Null);
}

#[tokio::test]
async fn saving_checks_the_values_and_never_takes_a_program_path() {
    let page = Page::start().await;
    let config = MeetingsConfig {
        auto_start: true,
        settle_seconds: 7,
        language: "uk".into(),
        transcriber: Some(PathBuf::from("/bin/evil")),
        ..MeetingsConfig::default()
    };
    let saved = page
        .request(reqwest::Method::PUT, "/settings/meetings")
        .json(&config)
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200, "{}", saved.text().await.unwrap());
    let on_disk = page.saved().meetings;
    assert!(on_disk.auto_start);
    assert_eq!(on_disk.settle_seconds, 7);
    assert_eq!(on_disk.language, "uk");
    assert_eq!(
        on_disk.transcriber,
        Some(page.dir.path().join("better-kumo-transcriber"))
    );
    assert!(page.relay.meetings_auto_start());

    let too_long = page
        .request(reqwest::Method::PUT, "/settings/meetings")
        .json(&MeetingsConfig {
            settle_seconds: 9_999,
            ..MeetingsConfig::default()
        })
        .send()
        .await
        .unwrap();
    assert_eq!(too_long.status(), 400);
    assert_eq!(page.saved().meetings.settle_seconds, 7);
}

#[tokio::test]
async fn a_recording_starts_and_stops_from_the_page_only() {
    let page = Page::start().await;
    let forged = page
        .request(reqwest::Method::POST, "/settings/meetings/recording")
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 403);
    assert!(!page.relay.meeting_recording());

    let started = page
        .request(reqwest::Method::POST, "/settings/meetings/recording")
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 200);
    let body: Value = started.json().await.unwrap();
    assert_eq!(body["recording"], true);
    assert!(page.relay.meeting_recording());

    let stopped = page
        .request(reqwest::Method::DELETE, "/settings/meetings/recording")
        .send()
        .await
        .unwrap();
    assert_eq!(stopped.status(), 200);
    let body: Value = stopped.json().await.unwrap();
    assert_eq!(body["recording"], false);
    assert!(!page.relay.meeting_recording());
}
