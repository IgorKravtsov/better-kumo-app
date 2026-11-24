//! Mirrors the desktop composer's Giphy picker (`SpacePage-*.js`, read 2026-11-24) so GIFs sent from the
//! phone look exactly like the desktop's to every other Kumospace client.

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::error::{CoreError, Result};
use crate::http::HttpClient;

/// Only used when Kumospace's web app can't be fetched or parsed (copy read 2026-11-24).
const FALLBACK_GIPHY_KEY: &str = "ZtsAdUbDe9TR0ZJm2GmG6Iz2lgNsfU5f";
const KUMOSPACE_WEB_URL: &str = "https://www.kumospace.com";
const KEY_MARKER: &str = "REACT_APP_GIPHY_KEY:";
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

/// Giphy client using the key Kumospace's own web app currently ships, so GIF search keeps working when they
/// rotate it.
pub struct Giphy {
    http: HttpClient,
    key: Mutex<Option<String>>,
}

impl Giphy {
    pub fn new(http: HttpClient) -> Self {
        Self {
            http,
            key: Mutex::new(None),
        }
    }

    pub async fn search(&self, query: &str, offset: u32) -> Result<GifPage> {
        let mut key = self.key(false).await;
        let mut page = match fetch_page(&self.http, &key, query, offset).await? {
            Some(page) => page,
            None => {
                key = self.key(true).await;
                fetch_page(&self.http, &key, query, offset)
                    .await?
                    .ok_or_else(|| CoreError::Invalid("Giphy rejected the key".into()))?
            }
        };
        // The pickers only page when a last cell scrolls into view, so a page with nothing usable would stall them.
        for _ in 0..MAX_EMPTY_PAGES {
            if !page.gifs.is_empty() || page.reached_end {
                break;
            }
            let Some(next) = fetch_page(&self.http, &key, query, page.next_offset).await? else {
                break;
            };
            page = next;
        }
        Ok(page)
    }

    async fn key(&self, refresh: bool) -> String {
        let mut cached = self.key.lock().await;
        if !refresh && let Some(key) = cached.as_ref() {
            return key.clone();
        }
        let key = web_app_key(&self.http)
            .await
            .unwrap_or_else(|| FALLBACK_GIPHY_KEY.into());
        *cached = Some(key.clone());
        key
    }
}

const MAX_EMPTY_PAGES: usize = 3;

/// `None` when Giphy refuses the key.
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

/// The key from the env chunk of `https://www.kumospace.com/app` (`useFetch-*.js` as of 2026-11-24, but any chunk
/// listed on the page is searched, since chunk names change between releases).
async fn web_app_key(http: &HttpClient) -> Option<String> {
    let html = get_text(http, &format!("{KUMOSPACE_WEB_URL}/app")).await?;
    let mut chunks = asset_scripts(&html);
    chunks.sort_by_key(|path| !path.starts_with("/assets/useFetch-"));
    for path in chunks {
        let js = get_text(http, &format!("{KUMOSPACE_WEB_URL}{path}")).await?;
        if let Some(key) = find_key(&js) {
            return Some(key);
        }
    }
    None
}

async fn get_text(http: &HttpClient, url: &str) -> Option<String> {
    http.get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()
}

fn asset_scripts(html: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("/assets/") {
        let tail = &rest[start..];
        let len = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || "/._-".contains(c)))
            .unwrap_or(tail.len());
        let path = &tail[..len];
        if path.ends_with(".js") && !found.iter().any(|p| p == path) {
            found.push(path.to_string());
        }
        rest = &tail[len.max(1)..];
    }
    found
}

fn find_key(js: &str) -> Option<String> {
    let rest = &js[js.find(KEY_MARKER)? + KEY_MARKER.len()..];
    let quote = rest.chars().next().filter(|c| "`\"'".contains(*c))?;
    let body = &rest[1..];
    let key = &body[..body.find(quote)?];
    let valid = (16..=64).contains(&key.len()) && key.chars().all(|c| c.is_ascii_alphanumeric());
    valid.then(|| key.to_string())
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
    fn finds_key_in_web_app() {
        let html = r#"<link rel="modulepreload" href="/assets/Auth-VO2cnKqP.js"><script type="module" src="/assets/main-DFV3TZ-0.js"></script><link href="/assets/useFetch-CAj8CxNN.js"><link href="/assets/index-x.css">"#;
        assert_eq!(
            asset_scripts(html),
            vec![
                "/assets/Auth-VO2cnKqP.js",
                "/assets/main-DFV3TZ-0.js",
                "/assets/useFetch-CAj8CxNN.js"
            ]
        );
        let js = "e={REACT_APP_API:`x`,REACT_APP_GIPHY_KEY:`ZtsAdUbDe9TR0ZJm2GmG6Iz2lgNsfU5f`,REACT_APP_MICROSOFT:`y`}";
        assert_eq!(
            find_key(js).as_deref(),
            Some("ZtsAdUbDe9TR0ZJm2GmG6Iz2lgNsfU5f")
        );
        assert_eq!(
            find_key(r#"REACT_APP_GIPHY_KEY:"abcdefghijklmnop1234""#).as_deref(),
            Some("abcdefghijklmnop1234")
        );
        assert_eq!(find_key("REACT_APP_GIPHY_KEY:e.key"), None);
        assert_eq!(find_key("REACT_APP_GIPHY_KEY:``"), None);
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
