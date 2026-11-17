use std::sync::Arc;

use kumo_relay::config::NtfyConfig;
use kumo_relay::desktop::replay::Replay;
use kumo_relay::desktop::{AlertSink, Desktop};
use kumo_relay::network::PairAddress;
use kumo_relay::push::Ntfy;
use kumo_relay::{Config, Relay, RelayParts};
use serde_json::{Value, json};

const SECRET: &str = "demo-secret-not-a-real-one-0123456789";
const PORT: u16 = 8788;

fn person(
    id: &str,
    name: &str,
    at: (f64, f64),
    zone: Option<&str>,
    availability: &str,
    message: &str,
) -> Value {
    json!({
        "id": id, "uid": format!("uid-{id}"), "name": name, "avatar": "",
        "x": at.0, "y": at.1, "zoneId": zone, "self": id == "c-me",
        "availability": availability, "idle": false, "message": message
    })
}

fn office() -> Value {
    let people = vec![
        person(
            "c-me",
            "Alex Rivera",
            (690.0, 250.0),
            Some("zone-eng"),
            "available",
            "",
        ),
        person(
            "c-1",
            "Maya Chen",
            (930.0, 270.0),
            Some("zone-eng"),
            "focusing",
            "Deep work until 3",
        ),
        person(
            "c-2",
            "Sam Okafor",
            (800.0, 440.0),
            Some("zone-eng"),
            "available",
            "",
        ),
        person(
            "c-3",
            "Lena Novak",
            (240.0, 250.0),
            Some("zone-design"),
            "available",
            "Reviewing mocks",
        ),
        person(
            "c-4",
            "Jonas Berg",
            (430.0, 430.0),
            Some("zone-design"),
            "away",
            "Lunch",
        ),
        person(
            "c-5",
            "Priya Shah",
            (1250.0, 260.0),
            Some("zone-meet"),
            "available",
            "In a call",
        ),
        person(
            "c-6",
            "Tom Weber",
            (1490.0, 430.0),
            Some("zone-meet"),
            "available",
            "",
        ),
        person(
            "c-7",
            "Ana Costa",
            (520.0, 920.0),
            Some("zone-kitchen"),
            "available",
            "Coffee?",
        ),
        person("c-8", "Leo Martin", (1150.0, 950.0), None, "away", ""),
    ];
    json!({
        "floorId": "floor-1",
        "width": 1800,
        "height": 1200,
        "floors": [{ "id": "floor-1", "name": "Main floor" }, { "id": "floor-2", "name": "Rooftop" }],
        "self": { "id": "c-me", "x": 690.0, "y": 250.0, "zoneId": "zone-eng", "availability": "available", "message": "" },
        "zones": [
            { "id": "zone-design", "name": "Design", "x": 150, "y": 150, "w": 380, "h": 380, "open": true },
            { "id": "zone-eng", "name": "Engineering", "x": 600, "y": 150, "w": 420, "h": 380, "open": true },
            { "id": "zone-meet", "name": "Meeting room", "x": 1150, "y": 150, "w": 450, "h": 380, "open": false },
            { "id": "zone-kitchen", "name": "Kitchen", "x": 300, "y": 750, "w": 500, "h": 320, "open": true }
        ],
        "people": people,
        "audio": true,
        "video": false
    })
}

struct Quiet;

impl AlertSink for Quiet {
    fn alert(&self, _alert: kumo_core::chat::alert::Alert) {}
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join("kumo-demo-relay");
    std::fs::create_dir_all(&dir)?;
    let mut config = Config::template(SECRET, "demo-office");
    config.listen = format!("0.0.0.0:{PORT}");
    config.data_dir = Some(dir.clone());
    let ntfy = NtfyConfig {
        server: "https://ntfy.sh".into(),
        topic: "my-kumo-alerts".into(),
        enabled: true,
    };
    config.ntfy = Some(ntfy.clone());
    config.save(&dir.join("relay.toml"))?;

    let replay = Arc::new(Replay::new());
    replay.answer("window.__kumo.state()", office());
    replay.answer("window.__kumo.send(", json!(true));
    replay.answer("window.__kumo.broadcastShown()", json!(false));
    let desktop = Desktop::with_dev_tools(replay);
    let relay = Relay::new(RelayParts {
        uid: "uid-me".into(),
        data_dir: dir.clone(),
        apns: None,
        ntfy: Some(Ntfy::new(ntfy, reqwest::Client::new())),
        config_path: Some(dir.join("relay.toml")),
        desktop: Some(desktop.clone()),
    });
    desktop.spawn(Arc::new(Quiet));

    if let Some(path) = std::env::args().nth(1) {
        let addresses = [
            PairAddress {
                label: "Tailscale",
                url: "http://100.101.102.103:8787".to_string(),
            },
            PairAddress {
                label: "Same Wi-Fi",
                url: "http://192.168.1.20:8787".to_string(),
            },
        ];
        std::fs::write(
            &path,
            kumo_relay::pair_page::render_for(&config, &addresses),
        )?;
        println!("pair page: {path}");
    }
    println!(
        "pair link: {}",
        kumo_relay::pair_link(&config, &format!("http://127.0.0.1:{PORT}"))
    );
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", PORT)).await?;
    kumo_relay::serve(listener, relay, config, std::future::pending()).await?;
    Ok(())
}
