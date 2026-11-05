//! A forgiving tag scanner for EPUB's XML/XHTML. Real-world EPUBs are often not well-formed
//! XML (stray `&nbsp;`, unclosed tags), so this never errors: it just yields what it can.

use super::normalize;

#[derive(Debug, PartialEq, Eq)]
pub enum Tok<'a> {
    Open { name: String, attrs: Vec<(String, String)>, self_close: bool },
    Close { name: String },
    Text(&'a str),
}

fn find(s: &str, from: usize, pat: &str) -> Option<usize> {
    s.get(from..)?.find(pat).map(|p| p + from)
}

/// Byte index of the `>` closing a tag that starts at `from`, skipping quoted attribute values.
fn tag_end(b: &[u8], from: usize) -> usize {
    let mut quote = None;
    for (i, &c) in b.iter().enumerate().skip(from) {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if c == b'>' => return i,
            None => {}
        }
    }
    b.len()
}

/// Lowercased name without its namespace prefix: `opf:item` → `item`.
fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_ascii_lowercase()
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut out = Vec::new();
    loop {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        let start = i;
        while i < n && !chars[i].is_whitespace() && chars[i] != '=' {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < n && chars[i] == '=' {
            i += 1;
            while i < n && chars[i].is_whitespace() {
                i += 1;
            }
            if i < n && (chars[i] == '"' || chars[i] == '\'') {
                let q = chars[i];
                i += 1;
                let vs = i;
                while i < n && chars[i] != q {
                    i += 1;
                }
                value = chars[vs..i].iter().collect();
                i += 1;
            } else {
                let vs = i;
                while i < n && !chars[i].is_whitespace() {
                    i += 1;
                }
                value = chars[vs..i].iter().collect();
            }
        }
        if name.is_empty() {
            i += 1; // stray '=' — step past it
            continue;
        }
        out.push((local(&name), unescape(&value)));
    }
    out
}

pub fn tokenize(s: &str) -> Vec<Tok<'_>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut text_start = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if text_start < i {
            out.push(Tok::Text(&s[text_start..i]));
        }
        let rest = &s[i..];
        if rest.starts_with("<!--") {
            i = find(s, i + 4, "-->").map_or(b.len(), |p| p + 3);
        } else if rest.starts_with("<![CDATA[") {
            let end = find(s, i + 9, "]]>").unwrap_or(b.len());
            out.push(Tok::Text(&s[i + 9..end]));
            i = (end + 3).min(b.len());
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            i = find(s, i, ">").map_or(b.len(), |p| p + 1);
        } else {
            let end = tag_end(b, i + 1);
            let inner = s[i + 1..end].trim();
            i = (end + 1).min(b.len());
            if let Some(name) = inner.strip_prefix('/') {
                out.push(Tok::Close { name: local(name.trim()) });
            } else {
                let self_close = inner.ends_with('/');
                let inner = inner.trim_end_matches('/');
                let split = inner.find(char::is_whitespace).unwrap_or(inner.len());
                let name = local(&inner[..split]);
                if !name.is_empty() {
                    out.push(Tok::Open { name, attrs: parse_attrs(&inner[split..]), self_close });
                }
            }
        }
        text_start = i;
    }
    if text_start < b.len() {
        out.push(Tok::Text(&s[text_start..]));
    }
    out
}

pub fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" | "ensp" | "emsp" | "thinsp" => ' ',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "middot" => '·',
        "copy" => '©',
        _ => return None,
    })
}

/// Decode `&...;` entities; anything unknown is kept verbatim.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        match after.find(';').filter(|&p| p <= 10).and_then(|p| entity(&after[..p]).map(|c| (p, c))) {
            Some((p, c)) => {
                out.push(c);
                rest = &after[p + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

const BLOCK: &[&str] = &[
    "p", "div", "br", "h1", "h2", "h3", "h4", "h5", "h6", "li", "tr", "blockquote", "section",
    "article", "hr", "dd", "dt", "pre", "header", "footer", "aside", "figcaption", "body",
];
const SKIP: &[&str] = &["head", "script", "style", "svg", "rt", "rp"];

/// Plain-text paragraphs of an (X)HTML document.
pub fn html_to_paragraphs(s: &str) -> Vec<String> {
    let mut paras = Vec::new();
    let mut cur = String::new();
    let mut skip = 0usize;
    let mut flush = |cur: &mut String| {
        let p = normalize(cur);
        if !p.is_empty() {
            paras.push(p);
        }
        cur.clear();
    };
    for tok in tokenize(s) {
        match tok {
            Tok::Open { name, self_close, .. } => {
                if SKIP.contains(&name.as_str()) {
                    if !self_close {
                        skip += 1;
                    }
                } else if BLOCK.contains(&name.as_str()) {
                    flush(&mut cur);
                }
            }
            Tok::Close { name } => {
                if SKIP.contains(&name.as_str()) {
                    skip = skip.saturating_sub(1);
                } else if BLOCK.contains(&name.as_str()) {
                    flush(&mut cur);
                }
            }
            Tok::Text(t) if skip == 0 => cur.push_str(&unescape(t)),
            Tok::Text(_) => {}
        }
    }
    flush(&mut cur);
    paras
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_paragraphs_from_xhtml() {
        let html = r#"<?xml version="1.0"?><!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Ch 1</title><style>p{}</style></head>
<body><h1>第一章</h1><!-- note --><p class="x">他推开门，
屋里&nbsp;一片漆黑&hellip;</p><p>Tom &amp; Jerry<br/>said &#8220;hi&#x201D;</p>
<p>a &bogus; b</p></body></html>"#;
        assert_eq!(
            html_to_paragraphs(html),
            ["第一章", "他推开门，屋里 一片漆黑…", "Tom & Jerry", "said “hi”", "a &bogus; b"]
        );
    }

    #[test]
    fn reads_namespaced_attrs() {
        let toks = tokenize(r#"<opf:item id='c1' href="text/ch%201.xhtml" media-type=application/xhtml+xml />"#);
        let Tok::Open { name, attrs, self_close } = &toks[0] else { panic!() };
        assert_eq!(name, "item");
        assert!(self_close);
        assert_eq!(attr(attrs, "href"), Some("text/ch%201.xhtml"));
        assert_eq!(attr(attrs, "media-type"), Some("application/xhtml+xml"));
    }
}
