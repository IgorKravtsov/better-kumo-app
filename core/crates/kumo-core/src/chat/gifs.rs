//! Mirrors the desktop composer's Giphy picker (`SpacePage-*.js`, read 2026-11-24) so GIFs sent from the
//! phone look exactly like the desktop's to every other Kumospace client.

use serde::Deserialize;

use crate::error::Result;
use crate::http::HttpClient;

/// Kumospace's public Giphy key (`REACT_APP_GIPHY_KEY` in the web bundle).
pub const GIPHY_API_KEY: &str = "ZtsAdUbDe9TR0ZJm2GmG6Iz2lgNsfU5f";
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

pub async fn search_gifs(http: &HttpClient, query: &str, offset: u32) -> Result<GifPage> {
    // The pickers only page when a last cell scrolls into view, so a page with nothing usable would stall them.
    let mut page = fetch_page(http, query, offset).await?;
    for _ in 0..MAX_EMPTY_PAGES {
        if !page.gifs.is_empty() || page.reached_end {
            break;
        }
        page = fetch_page(http, query, page.next_offset).await?;
    }
    Ok(page)
}

const MAX_EMPTY_PAGES: usize = 3;

async fn fetch_page(http: &HttpClient, query: &str, offset: u32) -> Result<GifPage> {
    let query = query.trim();
    let offset_param = offset.to_string();
    let limit = GIF_PAGE_SIZE.to_string();
    let mut params = vec![
        ("api_key", GIPHY_API_KEY),
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
    let page: Page = http
        .get(format!("{GIPHY_BASE_URL}/{endpoint}"))
        .query(&params)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(page.into_gif_page(offset))
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
