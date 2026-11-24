//! Mirrors the desktop composer's Giphy picker (`SpacePage-*.js`, read 2026-11-24) so GIFs sent from the
//! phone look exactly like the desktop's to every other Kumospace client.

use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Mutex;
use url::Url;

use crate::error::{CoreError, Result};
use crate::http::HttpClient;

/// Only used when Kumospace's web app can't be fetched or parsed (copy read 2026-11-24).
const FALLBACK_GIPHY_KEY: &str = "ZtsAdUbDe9TR0ZJm2GmG6Iz2lgNsfU5f";
const KUMOSPACE_APP_URL: &str = "https://www.kumospace.com/app";
const KEY_NAME: &str = "REACT_APP_GIPHY_KEY";
const GIPHY_BASE_URL: &str = "https://api.giphy.com/v1/gifs";
pub const GIF_PAGE_SIZE: u32 = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gif {
    pub id: String,
    pub title: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub preview_url: String,
    pub preview_width: u32,
    pub preview_height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GifPage {
    pub gifs: Vec<Gif>,
    /// Giphy's offsets count raw results, including ones dropped here for lacking a usable rendition.
    pub next_offset: u32,
    pub reached_end: bool,
}

impl Gif {
    pub fn markdown(&self) -> String {
        let title: String = self
            .title
            .chars()
            .map(|c| match c {
                '[' | ']' | '`' | '\n' | '\r' => ' ',
                c => c,
            })
            .collect();
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        let title = if title.is_empty() {
            "GIF".into()
        } else {
            title
        };
        let url = self.url.replace('(', "%28").replace(')', "%29");
        format!("![{title} {},{}]({url})", self.width, self.height)
    }
}

pub struct Giphy {
    http: HttpClient,
    key: Mutex<Option<ResolvedKey>>,
}

struct ResolvedKey {
    key: String,
    at: Instant,
}

impl Giphy {
    pub fn new(http: HttpClient) -> Self {
        Self {
            http,
            key: Mutex::new(None),
        }
    }

    pub async fn search(&self, query: &str, offset: u32) -> Result<GifPage> {
        let mut key = self.key().await;
        let mut refreshed = false;
        let mut page = self.page(&mut key, &mut refreshed, query, offset).await?;
        // The pickers only page when a last cell scrolls into view, so a page with nothing usable would stall them.
        for _ in 0..MAX_EMPTY_PAGES {
            if !page.gifs.is_empty() || page.reached_end {
                break;
            }
            page = self
                .page(&mut key, &mut refreshed, query, page.next_offset)
                .await?;
        }
        Ok(page)
    }

    async fn page(
        &self,
        key: &mut String,
        refreshed: &mut bool,
        query: &str,
        offset: u32,
    ) -> Result<GifPage> {
        if let Some(page) = fetch_page(&self.http, key, query, offset).await? {
            return Ok(page);
        }
        if !*refreshed {
            *refreshed = true;
            *key = self.refresh(key).await;
            if let Some(page) = fetch_page(&self.http, key, query, offset).await? {
                return Ok(page);
            }
        }
        Err(CoreError::Invalid("Giphy refused the request".into()))
    }

    async fn key(&self) -> String {
        let mut cached = self.key.lock().await;
        if let Some(resolved) = cached.as_ref() {
            return resolved.key.clone();
        }
        let key = resolve_key(&self.http).await;
        *cached = Some(ResolvedKey {
            key: key.clone(),
            at: Instant::now(),
        });
        key
    }

    /// Searches that were all refused with the same key share one download, and a key that keeps being refused
    /// (or a 403 that is really rate limiting) can't make every search download Kumospace's web app again.
    async fn refresh(&self, refused: &str) -> String {
        let mut cached = self.key.lock().await;
        if let Some(resolved) = cached.as_ref()
            && (resolved.key != refused || resolved.at.elapsed() < REFRESH_COOLDOWN)
        {
            return resolved.key.clone();
        }
        let key = resolve_key(&self.http).await;
        *cached = Some(ResolvedKey {
            key: key.clone(),
            at: Instant::now(),
        });
        key
    }
}

const MAX_EMPTY_PAGES: usize = 3;
const REFRESH_COOLDOWN: Duration = Duration::from_secs(60);
const RESOLVE_DEADLINE: Duration = Duration::from_secs(6);
const MAX_CHUNKS: usize = 12;
const MAX_BODY_BYTES: usize = 3 * 1024 * 1024;

/// `None` when Giphy refuses the request (401/403).
async fn fetch_page(
    http: &HttpClient,
    key: &str,
    query: &str,
    offset: u32,
) -> Result<Option<GifPage>> {
    let query = query.trim();
    let offset_param = offset.to_string();
    let limit = GIF_PAGE_SIZE.to_string();
    let mut params = vec![
        ("api_key", key),
        ("rating", "pg-13"),
        ("offset", offset_param.as_str()),
        ("limit", limit.as_str()),
    ];
    let endpoint = if query.is_empty() {
        "trending"
    } else {
        params.push(("q", query));
        "search"
    };
    let res = http
        .get(format!("{GIPHY_BASE_URL}/{endpoint}"))
        .query(&params)
        .send()
        .await?;
    if matches!(res.status().as_u16(), 401 | 403) {
        return Ok(None);
    }
    let page: Page = res.error_for_status()?.json().await?;
    Ok(Some(page.into_gif_page(offset)))
}

async fn resolve_key(http: &HttpClient) -> String {
    tokio::time::timeout(RESOLVE_DEADLINE, web_app_key(http))
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| FALLBACK_GIPHY_KEY.into())
}

async fn web_app_key(http: &HttpClient) -> Option<String> {
    let page = Url::parse(KUMOSPACE_APP_URL).ok()?;
    let html = get_text(http, &page).await?;
    let mut chunks = script_urls(&page, &html);
    // Deployments can move the env literal to another chunk, so the rest are searched too.
    chunks.sort_by_key(|url| !url.path().starts_with("/assets/useFetch-"));
    for url in chunks.into_iter().take(MAX_CHUNKS) {
        if let Some(key) = get_text(http, &url).await.and_then(|js| find_key(&js)) {
            return Some(key);
        }
    }
    None
}

async fn get_text(http: &HttpClient, url: &Url) -> Option<String> {
    let mut res = http
        .get(url.as_str())
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).ok()
}

/// Same-site `.js` URLs from `src`/`href` attributes, resolved against the page.
fn script_urls(page: &Url, html: &str) -> Vec<Url> {
    let mut found: Vec<Url> = Vec::new();
    for attr in ["src=", "href="] {
        for (at, _) in html.match_indices(attr) {
            let rest = &html[at + attr.len()..];
            let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let Some(len) = rest[1..].find(quote) else {
                continue;
            };
            let Ok(url) = page.join(&rest[1..1 + len]) else {
                continue;
            };
            let same_site = url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|h| h == "kumospace.com" || h.ends_with(".kumospace.com"));
            if same_site && url.path().ends_with(".js") && !found.contains(&url) {
                found.push(url);
            }
        }
    }
    found
}

fn find_key(js: &str) -> Option<String> {
    js.match_indices(KEY_NAME).find_map(|(at, _)| {
        let rest = js[at + KEY_NAME.len()..].trim_start_matches(['"', '\'']);
        let rest = rest.trim_start().strip_prefix(':')?.trim_start();
        let quote = rest
            .chars()
            .next()
            .filter(|c| matches!(c, '`' | '"' | '\''))?;
        let body = &rest[1..];
        let key = &body[..body.find(quote)?];
        let valid =
            (16..=64).contains(&key.len()) && key.chars().all(|c| c.is_ascii_alphanumeric());
        valid.then(|| key.to_string())
    })
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    data: Vec<RawGif>,
}

impl Page {
    fn into_gif_page(self, offset: u32) -> GifPage {
        let raw = self.data.len() as u32;
        GifPage {
            gifs: self.data.into_iter().filter_map(RawGif::into_gif).collect(),
            next_offset: offset + raw,
            reached_end: raw == 0,
        }
    }
}

#[derive(Deserialize)]
struct RawGif {
    id: String,
    #[serde(default)]
    title: String,
    images: Images,
}

#[derive(Deserialize)]
struct Images {
    original: Option<Rendition>,
    fixed_width_downsampled: Option<Rendition>,
    fixed_width: Option<Rendition>,
    fixed_width_small: Option<Rendition>,
}

#[derive(Deserialize)]
struct Rendition {
    #[serde(default)]
    url: String,
    #[serde(default)]
    width: Number,
    #[serde(default)]
    height: Number,
}

/// Giphy sends sizes as strings ("480"); accept numbers too.
#[derive(Deserialize, Default)]
#[serde(untagged)]
enum Number {
    Text(String),
    Int(u32),
    #[default]
    Missing,
}

impl Number {
    fn value(&self) -> u32 {
        match self {
            Number::Text(s) => s.parse().unwrap_or(0),
            Number::Int(n) => *n,
            Number::Missing => 0,
        }
    }
}

impl RawGif {
    fn into_gif(self) -> Option<Gif> {
        let original = self.images.original.filter(|r| !r.url.is_empty())?;
        // Never preview with the original: a full-size animation per grid cell exhausts phone memory.
        let preview = [
            self.images.fixed_width_downsampled,
            self.images.fixed_width_small,
            self.images.fixed_width,
        ]
        .into_iter()
        .flatten()
        .find(|r| !r.url.is_empty())?;
        Some(Gif {
            id: self.id,
            title: self.title,
            url: original.url,
            width: original.width.value(),
            height: original.height.value(),
            preview_url: preview.url,
            preview_width: preview.width.value(),
            preview_height: preview.height.value(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::markdown::parse;

    #[test]
    fn decodes_giphy_page() {
        let json = r#"{"data":[{"id":"abc","title":"Happy Dance GIF","images":{
            "original":{"url":"https://media0.giphy.com/media/abc/giphy.gif?cid=1&rid=giphy.gif&ct=g","width":"480","height":"342"},
            "fixed_width_downsampled":{"url":"https://media0.giphy.com/media/abc/200w_d.gif","width":"200","height":"143"}}},
            {"id":"no-original","images":{}}],"pagination":{"offset":0}}"#;
        let page: Page = serde_json::from_str(json).unwrap();
        let page = page.into_gif_page(48);
        assert_eq!((page.next_offset, page.reached_end), (50, false));
        assert_eq!(page.gifs.len(), 1);
        let gif = &page.gifs[0];
        assert_eq!((gif.width, gif.height), (480, 342));
        assert_eq!((gif.preview_width, gif.preview_height), (200, 143));
        assert_eq!(
            gif.markdown(),
            "![Happy Dance GIF 480,342](https://media0.giphy.com/media/abc/giphy.gif?cid=1&rid=giphy.gif&ct=g)"
        );
        let parsed = parse(&gif.markdown());
        assert_eq!(parsed.images, vec![gif.url.clone()]);
    }

    #[test]
    fn finds_script_urls_on_the_app_page() {
        let page = Url::parse(KUMOSPACE_APP_URL).unwrap();
        let html = r#"<link rel="modulepreload" href="/assets/Auth-VO2cnKqP.js"><script type="module" src='/assets/main-DFV3TZ-0.js'></script>
            <link href="https://cdn.kumospace.com/assets/useFetch-CAj8CxNN.js?v=2"><script src="assets/rel-1.js"></script>
            <link href="/assets/index-x.css"><script src="https://evil.example/assets/x.js"></script><link href="/assets/Auth-VO2cnKqP.js">"#;
        let urls: Vec<String> = script_urls(&page, html)
            .iter()
            .map(Url::to_string)
            .collect();
        assert_eq!(
            urls,
            vec![
                "https://www.kumospace.com/assets/main-DFV3TZ-0.js",
                "https://www.kumospace.com/assets/rel-1.js",
                "https://www.kumospace.com/assets/Auth-VO2cnKqP.js",
                "https://cdn.kumospace.com/assets/useFetch-CAj8CxNN.js?v=2",
            ]
        );
    }

    #[test]
    fn finds_key_in_bundle_text() {
        let key = Some("ZtsAdUbDe9TR0ZJm2GmG6Iz2lgNsfU5f");
        let js = "e={REACT_APP_API:`x`,REACT_APP_GIPHY_KEY:`ZtsAdUbDe9TR0ZJm2GmG6Iz2lgNsfU5f`,REACT_APP_MICROSOFT:`y`}";
        assert_eq!(find_key(js).as_deref(), key);
        assert_eq!(
            find_key(r#"{"REACT_APP_GIPHY_KEY": "ZtsAdUbDe9TR0ZJm2GmG6Iz2lgNsfU5f"}"#).as_deref(),
            key
        );
        assert_eq!(
            find_key("new GiphyFetch(tn.REACT_APP_GIPHY_KEY??``);env={REACT_APP_GIPHY_KEY : 'ZtsAdUbDe9TR0ZJm2GmG6Iz2lgNsfU5f'}")
                .as_deref(),
            key
        );
        assert_eq!(find_key("REACT_APP_GIPHY_KEY:e.key"), None);
        assert_eq!(find_key("REACT_APP_GIPHY_KEY:``"), None);
        assert_eq!(find_key("REACT_APP_GIPHY_KEY:`short`"), None);
        assert_eq!(find_key("no key here"), None);
    }

    #[test]
    fn markdown_cleans_title() {
        let gif = Gif {
            id: "x".into(),
            title: " a [b]\nc ".into(),
            url: "https://media.giphy.com/x.gif".into(),
            width: 1,
            height: 2,
            preview_url: String::new(),
            preview_width: 0,
            preview_height: 0,
        };
        assert_eq!(
            gif.markdown(),
            "![a b c 1,2](https://media.giphy.com/x.gif)"
        );
        let blank = Gif {
            title: String::new(),
            ..gif
        };
        assert_eq!(
            blank.markdown(),
            "![GIF 1,2](https://media.giphy.com/x.gif)"
        );
    }
    #[test]
    fn markdown_survives_code_fences_and_parentheses() {
        let gif = Gif {
            id: "x".into(),
            title: "```a``` ```b```".into(),
            url: "https://media.giphy.com/x(1).gif".into(),
            width: 1,
            height: 2,
            preview_url: String::new(),
            preview_width: 0,
            preview_height: 0,
        };
        let markdown = gif.markdown();
        assert_eq!(markdown, "![a b 1,2](https://media.giphy.com/x%281%29.gif)");
        assert_eq!(
            parse(&markdown).images,
            vec!["https://media.giphy.com/x%281%29.gif".to_string()]
        );
    }
}
