use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub highlight: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text {
        text: String,
        style: Style,
    },
    Link {
        text: String,
        url: String,
        style: Style,
    },
    Mention {
        user_id: String,
        kind: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
    pub inlines: Vec<Inline>,
    pub images: Vec<String>,
}

pub fn parse(text: &str) -> Parsed {
    let mut out = Parsed::default();
    for (_, segment, code) in blocks(text) {
        if !code {
            scan(&bullets(segment), Style::default(), &mut out);
            continue;
        }
        let body = match segment.split_once('\n') {
            Some((lang, code)) if !lang.contains(' ') && lang.len() < 16 => code,
            _ => segment,
        }
        .trim_matches('\n');
        push_text(
            &mut out,
            body,
            Style {
                code: true,
                ..Style::default()
            },
        );
    }
    trim_edges(&mut out);
    out
}

/// Splits text at fenced code blocks: `(offset, segment, is_code)`, where a code segment is what sits between the fences.
fn blocks(text: &str) -> Vec<(usize, &str, bool)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(start) = text[at..].find("```").map(|s| at + s) {
        let Some(end) = text[start + 3..].find("```").map(|e| start + 3 + e) else {
            break;
        };
        out.push((at, &text[at..start], false));
        out.push((start + 3, &text[start + 3..end], true));
        at = end + 3;
    }
    out.push((at, &text[at..], false));
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub user_id: String,
    /// `user`, `here` or `channel`.
    pub kind: String,
    /// The URL-decoded `name=` field, empty when the token has none.
    pub name: String,
}

impl Mention {
    /// `@here` and `@channel` notify everyone in the channel, so they include every user.
    pub fn includes(&self, user_id: &str) -> bool {
        match self.kind.as_str() {
            "user" => !user_id.is_empty() && self.user_id == user_id,
            kind => kind == "here" || kind == "channel",
        }
    }
}

/// Every mention the chat would render, with its byte range in `text`. A mention token inside code, a link or a URL is
/// not a mention.
pub fn mentions(text: &str) -> Vec<(Range<usize>, Mention)> {
    fn find(text: &str, offset: usize, style: Style, out: &mut Vec<(Range<usize>, Mention)>) {
        each_token(text, style, |at, len, token| match token {
            Token::Mention(m) => out.push((offset + at..offset + at + len, m)),
            Token::Styled { inner, style } => {
                find(inner, offset + at + (len - inner.len()) / 2, style, out)
            }
            _ => {}
        });
    }
    let mut out = Vec::new();
    for (offset, segment, code) in blocks(text) {
        if !code {
            find(segment, offset, Style::default(), &mut out);
        }
    }
    out
}

pub fn mentions_user(text: &str, user_id: &str) -> bool {
    mentions(text).iter().any(|(_, m)| m.includes(user_id))
}

fn bullets(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let body = line.trim_start();
            let indent = &line[..line.len() - body.len()];
            match body
                .strip_prefix("- ")
                .or_else(|| body.strip_prefix("* "))
                .or_else(|| body.strip_prefix("+ "))
            {
                Some(item) => format!("{indent}• {item}"),
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn trim_edges(out: &mut Parsed) {
    if let Some(Inline::Text { text, .. }) = out.inlines.first_mut() {
        *text = text.trim_start().to_string();
    }
    if let Some(Inline::Text { text, .. }) = out.inlines.last_mut() {
        *text = text.trim_end().to_string();
    }
    out.inlines
        .retain(|i| !matches!(i, Inline::Text { text, .. } if text.is_empty()));
}

fn push_text(out: &mut Parsed, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text {
        text: last,
        style: last_style,
    }) = out.inlines.last_mut()
        && *last_style == style
    {
        last.push_str(text);
        return;
    }
    out.inlines.push(Inline::Text {
        text: text.to_string(),
        style,
    });
}

enum Token<'a> {
    Image { url: &'a str },
    Link { label: &'a str, url: &'a str },
    Url { url: &'a str },
    Mention(Mention),
    Styled { inner: &'a str, style: Style },
}

fn each_token<'a>(text: &'a str, style: Style, mut on: impl FnMut(usize, usize, Token<'a>)) {
    let mut i = 0;
    while i < text.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let prev = text[..i].chars().next_back();
        match token_at(&text[i..], prev, style) {
            Some((len, token)) => {
                on(i, len, token);
                i += len;
            }
            None => i += 1,
        }
    }
}

fn scan(text: &str, style: Style, out: &mut Parsed) {
    let mut plain_start = 0;
    each_token(text, style, |at, len, token| {
        push_text(out, &text[plain_start..at], style);
        emit(token, style, out);
        plain_start = at + len;
    });
    push_text(out, &text[plain_start..], style);
}

fn emit(token: Token<'_>, style: Style, out: &mut Parsed) {
    match token {
        Token::Image { url } => out.images.push(url.to_string()),
        Token::Link { label, url } => out.inlines.push(Inline::Link {
            text: label.to_string(),
            url: url.to_string(),
            style,
        }),
        Token::Url { url } => out.inlines.push(Inline::Link {
            text: url.to_string(),
            url: url.to_string(),
            style,
        }),
        Token::Mention(Mention { user_id, kind, .. }) => {
            out.inlines.push(Inline::Mention { user_id, kind })
        }
        Token::Styled { inner, style: s } => scan(inner, s, out),
    }
}

fn token_at<'a>(rest: &'a str, prev: Option<char>, style: Style) -> Option<(usize, Token<'a>)> {
    if style.code {
        return None;
    }
    if let Some(r) = rest.strip_prefix("![") {
        let (label_end, url, total) = bracket_link(r)?;
        let _ = label_end;
        return Some((2 + total, Token::Image { url }));
    }
    if let Some(r) = rest.strip_prefix('[') {
        let (label_end, url, total) = bracket_link(r)?;
        return Some((
            1 + total,
            Token::Link {
                label: &r[..label_end],
                url,
            },
        ));
    }
    if let Some(r) = rest.strip_prefix("@mention(") {
        let end = r.find(')')?;
        let inner = &r[..end];
        let field = |name: &str| {
            inner
                .split('&')
                .find_map(|kv| kv.strip_prefix(name))
                .unwrap_or_default()
                .to_string()
        };
        let kind = field("type=");
        return Some((
            9 + end + 1,
            Token::Mention(Mention {
                user_id: field("id="),
                kind: if kind.is_empty() { "user".into() } else { kind },
                name: percent_decode(&field("name=")),
            }),
        ));
    }
    if (rest.starts_with("https://") || rest.starts_with("http://"))
        && prev.is_none_or(|c| !c.is_alphanumeric())
    {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '<' || c == '>')
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches(['.', ',', ')', '!', '?', ';', ':']);
        return Some((url.len(), Token::Url { url }));
    }
    let boundary_before = prev.is_none_or(|c| !c.is_alphanumeric());
    for (marker, apply) in MARKERS {
        if !rest.starts_with(marker) || !boundary_before {
            continue;
        }
        let body = &rest[marker.len()..];
        if body.starts_with(char::is_whitespace) {
            continue;
        }
        let close = body.find(marker)?;
        let inner = &body[..close];
        if inner.is_empty()
            || inner.ends_with(char::is_whitespace)
            || inner.contains('\n') && *marker == "`"
        {
            continue;
        }
        let after = body[close + marker.len()..].chars().next();
        if after.is_some_and(|c| c.is_alphanumeric()) {
            continue;
        }
        return Some((
            marker.len() * 2 + close,
            Token::Styled {
                inner,
                style: apply(style),
            },
        ));
    }
    None
}

type Apply = fn(Style) -> Style;

const MARKERS: &[(&str, Apply)] = &[
    ("`", |s| Style { code: true, ..s }),
    ("***", |s| Style {
        bold: true,
        italic: true,
        ..s
    }),
    ("___", |s| Style {
        bold: true,
        italic: true,
        ..s
    }),
    ("**", |s| Style { bold: true, ..s }),
    ("__", |s| Style { bold: true, ..s }),
    ("~~", |s| Style { strike: true, ..s }),
    ("==", |s| Style {
        highlight: true,
        ..s
    }),
    ("*", |s| Style { italic: true, ..s }),
    ("_", |s| Style { italic: true, ..s }),
];

/// One-line previews (chat list, notifications) show embedded images, which are GIFs in Kumospace, as "GIF".
pub fn summarize_images(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("![") {
        out.push_str(&rest[..start]);
        match bracket_link(&rest[start + 2..]) {
            Some((_, _, total)) => {
                out.push_str("GIF");
                rest = &rest[start + 2 + total..];
            }
            None => {
                out.push_str("![");
                rest = &rest[start + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
                continue;
            }
            (b'+', _) => out.push(b' '),
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn bracket_link(r: &str) -> Option<(usize, &str, usize)> {
    let label_end = r.find("](")?;
    if r[..label_end].contains('\n') {
        return None;
    }
    let url_part = &r[label_end + 2..];
    let url_end = url_part.find(')')?;
    let url = url_part[..url_end].trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    Some((label_end, url, label_end + 2 + url_end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str, style: Style) -> Inline {
        Inline::Text {
            text: t.into(),
            style,
        }
    }

    #[test]
    fn plain_and_styles() {
        let p = parse("hi **bold** and *it* ~~no~~ `x*y`");
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        assert_eq!(p.inlines[0], text("hi ", Style::default()));
        assert_eq!(p.inlines[1], text("bold", bold));
        assert!(
            p.inlines
                .iter()
                .any(|i| matches!(i, Inline::Text { text, style } if text == "x*y" && style.code))
        );
        assert!(
            p.inlines
                .iter()
                .any(|i| matches!(i, Inline::Text { text, style } if text == "no" && style.strike))
        );
    }

    #[test]
    fn snake_case_is_not_italic() {
        let p = parse("call some_function_name now");
        assert_eq!(
            p.inlines,
            vec![text("call some_function_name now", Style::default())]
        );
    }

    #[test]
    fn summarizes_images_for_previews() {
        assert_eq!(
            summarize_images("![Dance GIF 250,193](https://media.giphy.com/a.gif)"),
            "GIF"
        );
        assert_eq!(
            summarize_images("look ![a 1,2](https://x.io/a.gif) and ![b](not-a-url) !["),
            "look GIF and ![b](not-a-url) !["
        );
    }

    #[test]
    fn images_links_mentions() {
        let p = parse("![GIF 480,342](https://media.giphy.com/a.gif)");
        assert_eq!(p.images, vec!["https://media.giphy.com/a.gif".to_string()]);
        assert!(p.inlines.is_empty());

        let p = parse("see [docs](https://x.io/a) or https://y.io/b. @mention(type=user&id=u1)!");
        assert!(
            matches!(&p.inlines[1], Inline::Link { text, url, .. } if text == "docs" && url == "https://x.io/a")
        );
        assert!(matches!(&p.inlines[3], Inline::Link { url, .. } if url == "https://y.io/b"));
        assert!(
            matches!(&p.inlines[5], Inline::Mention { user_id, kind } if user_id == "u1" && kind == "user")
        );
    }

    #[test]
    fn bold_italic_and_highlight() {
        let p = parse("***both*** and ==marked==");
        assert_eq!(
            p.inlines,
            vec![
                text(
                    "both",
                    Style {
                        bold: true,
                        italic: true,
                        ..Style::default()
                    }
                ),
                text(" and ", Style::default()),
                text(
                    "marked",
                    Style {
                        highlight: true,
                        ..Style::default()
                    }
                ),
            ]
        );
        assert_eq!(
            parse("a == b").inlines,
            vec![text("a == b", Style::default())]
        );
    }

    #[test]
    fn list_items_become_bullets() {
        let p = parse("todo:\n- one\n  * two\n1. three\n```\n- code\n```");
        assert_eq!(
            p.inlines[0],
            text("todo:\n• one\n  • two\n1. three\n", Style::default())
        );
        assert!(matches!(&p.inlines[1], Inline::Text { text, .. } if text == "- code"));
    }

    #[test]
    fn code_blocks() {
        let p = parse("look:\n```js\nlet a = **1**;\n```\ndone");
        assert!(p.inlines.iter().any(
            |i| matches!(i, Inline::Text { text, style } if style.code && text.contains("**1**"))
        ));
    }

    #[test]
    fn mentions_of_me_in_every_wire_format_but_not_in_code_or_urls() {
        for text in [
            "@mention(type=user&id=me)",
            "@mention(id=me)",
            "hi @mention(type=user&id=me&name=Me%20Too)!",
            "@mention(type=user&name=Me&id=me)",
            "@mention(type=here)",
            "@mention(type=channel)",
            "**@mention(type=user&id=me&name=Me)**",
            "```\ncode\n``` then @mention(type=user&id=me)",
        ] {
            assert!(mentions_user(text, "me"), "{text}");
        }
        for text in [
            "@mention(type=user&id=bo&name=Bo)",
            "@mention(type=user&id=meg)",
            "@mention(type=user&id=me",
            "`@mention(type=user&id=me)`",
            "```\n@mention(type=here)\n```",
            "https://x.io/?q=@mention(type=user&id=me)&p=2",
            "[@mention(id=me)](https://x.io/a)",
            "![@mention(id=me)](https://x.io/a.gif)",
            "plain @me text",
        ] {
            assert!(!mentions_user(text, "me"), "{text}");
        }
        assert!(!mentions_user("@mention(type=user&id=)", ""));
        let found = mentions("a @mention(type=user&id=u1&name=Bo+B%C3%B8) b");
        assert_eq!(
            found,
            vec![(
                2..43,
                Mention {
                    user_id: "u1".into(),
                    kind: "user".into(),
                    name: "Bo Bø".into()
                }
            )]
        );
    }

    #[test]
    fn mentions_agree_with_the_rendered_message() {
        for text in [
            "hi @mention(type=user&id=u1) and `@mention(id=u2)` ~~@mention(type=here)~~",
            "- @mention(id=u1)\n* *@mention(id=u2)*\n```js\n@mention(id=u3)\n```\n@mention(type=channel)",
            "https://x.io/@mention(id=u1) [@mention(id=u2)](https://y.io) @mention(id=u3).",
            "```unclosed @mention(id=u1)",
            "日本 @mention(id=u1) ==@mention(id=u2)==",
        ] {
            let rendered: Vec<_> = parse(text)
                .inlines
                .into_iter()
                .filter_map(|i| match i {
                    Inline::Mention { user_id, kind } => Some((user_id, kind)),
                    _ => None,
                })
                .collect();
            let found: Vec<_> = mentions(text)
                .into_iter()
                .map(|(range, m)| {
                    assert!(text[range].starts_with("@mention("));
                    (m.user_id, m.kind)
                })
                .collect();
            assert_eq!(found, rendered, "{text}");
        }
    }
}
