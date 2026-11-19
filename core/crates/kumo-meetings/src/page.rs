use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use crate::audio::{Chunk, Speaker};
use crate::detector::Group;

pub const PAGE_JS: &str = include_str!("page.js");
pub const PAGE_VERSION: u32 = 3;

pub trait MeetingPage: Send + Sync {
    fn evaluate<'a>(&'a self, expression: &'a str) -> BoxFuture<'a, anyhow::Result<Value>>;
    fn announce<'a>(&'a self, text: &'a str) -> BoxFuture<'a, anyhow::Result<()>>;
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Person {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PageTick {
    pub in_call: bool,
    pub me: Option<Person>,
    pub people: Vec<Person>,
    pub zone: String,
    pub capturing: usize,
    pub error: Option<String>,
    pub chunks: Vec<Chunk>,
}

impl PageTick {
    pub fn earshot(&self) -> Group {
        self.people.iter().map(|p| p.id.clone()).collect()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTick {
    in_call: bool,
    me: Option<Person>,
    #[serde(default)]
    people: Vec<Person>,
    #[serde(default)]
    zone: String,
    #[serde(default)]
    capturing: usize,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    chunks: Vec<RawChunk>,
}

#[derive(Deserialize)]
struct RawChunk {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    me: bool,
    at: i64,
    pcm: String,
}

pub fn parse_tick(value: Value) -> anyhow::Result<PageTick> {
    let raw: RawTick =
        serde_json::from_value(value).context("unexpected meeting read from the page")?;
    let chunks = raw
        .chunks
        .into_iter()
        .filter_map(|c| {
            let bytes = STANDARD.decode(c.pcm).ok()?;
            let samples = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect();
            Some(Chunk {
                speaker: Speaker {
                    id: c.id,
                    name: c.name,
                    me: c.me,
                },
                at_ms: c.at,
                samples,
            })
        })
        .collect();
    Ok(PageTick {
        in_call: raw.in_call,
        me: raw.me,
        people: raw.people,
        zone: raw.zone,
        capturing: raw.capturing,
        error: raw.error,
        chunks,
    })
}

pub async fn read(page: &dyn MeetingPage, capture: bool) -> anyhow::Result<PageTick> {
    let expression = format!(
        "window.__kumoMeet && window.__kumoMeet.v === {PAGE_VERSION} ? window.__kumoMeet.tick({capture}) : null"
    );
    let mut value = page.evaluate(&expression).await?;
    if value.is_null() {
        page.evaluate(PAGE_JS).await?;
        value = page.evaluate(&expression).await?;
    }
    if value.is_null() {
        return Ok(PageTick::default());
    }
    parse_tick(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_people_heard_and_their_audio() {
        let pcm = STANDARD.encode([1u8, 0, 0xff, 0xff, 0x00, 0x80]);
        let tick = parse_tick(json!({
            "inCall": true,
            "me": { "id": "u-me", "name": "Igor" },
            "people": [{ "id": "u-anna", "name": "Anna" }, { "id": "u-bob", "name": "Bob" }],
            "zone": "Kitchen",
            "capturing": 3,
            "error": null,
            "chunks": [{ "id": "u-anna", "name": "Anna", "me": false, "at": 1000, "pcm": pcm }]
        }))
        .unwrap();
        assert!(tick.in_call);
        assert_eq!(
            tick.earshot().into_iter().collect::<Vec<_>>(),
            vec!["u-anna", "u-bob"]
        );
        assert_eq!(tick.zone, "Kitchen");
        assert_eq!(tick.chunks.len(), 1);
        assert_eq!(tick.chunks[0].samples, vec![1, -1, i16::MIN]);
        assert_eq!(tick.chunks[0].speaker.name, "Anna");
    }

    #[test]
    fn the_rust_side_asks_for_the_version_page_js_installs() {
        assert!(PAGE_JS.contains(&format!(
            "window.__kumoMeet.v === {PAGE_VERSION}) return true;"
        )));
        assert!(PAGE_JS.contains(&format!("    v: {PAGE_VERSION},\n")));
    }

    struct OldPage {
        upgraded: std::sync::Mutex<bool>,
        expressions: std::sync::Mutex<Vec<String>>,
    }

    impl MeetingPage for OldPage {
        fn evaluate<'a>(&'a self, expression: &'a str) -> BoxFuture<'a, anyhow::Result<Value>> {
            Box::pin(async move {
                self.expressions
                    .lock()
                    .unwrap()
                    .push(expression.to_string());
                let mut upgraded = self.upgraded.lock().unwrap();
                if expression == PAGE_JS {
                    *upgraded = true;
                    return Ok(Value::Bool(true));
                }
                let asks_current = expression.contains(&format!("v === {PAGE_VERSION}"));
                Ok(if asks_current && !*upgraded {
                    Value::Null
                } else {
                    json!({ "inCall": false, "me": null })
                })
            })
        }

        fn announce<'a>(&'a self, _: &'a str) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn an_older_script_in_the_page_is_replaced() {
        let page = OldPage {
            upgraded: std::sync::Mutex::new(false),
            expressions: std::sync::Mutex::new(Vec::new()),
        };
        read(&page, true).await.unwrap();
        assert!(*page.upgraded.lock().unwrap());
        assert_eq!(page.expressions.lock().unwrap().len(), 3);
        read(&page, true).await.unwrap();
        assert_eq!(page.expressions.lock().unwrap().len(), 4);
    }

    #[test]
    fn out_of_a_call_nobody_is_heard() {
        let tick = parse_tick(
            json!({ "inCall": false, "me": null, "people": [], "zone": "", "tracks": [] }),
        )
        .unwrap();
        assert!(!tick.in_call);
        assert!(tick.earshot().is_empty());
    }
}
