use std::{collections::HashMap, fs::File, io::Read, path::Path};

use anyhow::{Context, Result};
use zip::ZipArchive;

use super::markup::{attr, html_to_paragraphs, tokenize, Tok};

fn read_entry(zip: &mut ZipArchive<File>, name: &str) -> Result<String> {
    let mut entry = zip.by_name(name).with_context(|| format!("missing {name}"))?;
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Join an href onto the OPF's folder, resolving `.` and `..`.
fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or(href);
    let mut parts: Vec<&str> = if base.is_empty() { Vec::new() } else { base.split('/').collect() };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    percent_decode(&parts.join("/"))
}

pub fn load(path: &Path) -> Result<Vec<String>> {
    let mut zip = ZipArchive::new(File::open(path)?).context("not a valid epub (zip)")?;
    let container = read_entry(&mut zip, "META-INF/container.xml")?;
    let opf_path = tokenize(&container)
        .into_iter()
        .find_map(|t| match t {
            Tok::Open { name, attrs, .. } if name == "rootfile" => attr(&attrs, "full-path").map(String::from),
            _ => None,
        })
        .context("container.xml has no rootfile")?;
    let opf = read_entry(&mut zip, &opf_path)?;
    let base = opf_path.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();

    let mut manifest = HashMap::new();
    let mut spine = Vec::new();
    for tok in tokenize(&opf) {
        if let Tok::Open { name, attrs, .. } = tok {
            match name.as_str() {
                "item" => {
                    if let (Some(id), Some(href)) = (attr(&attrs, "id"), attr(&attrs, "href")) {
                        manifest.insert(id.to_string(), href.to_string());
                    }
                }
                "itemref" => spine.extend(attr(&attrs, "idref").map(String::from)),
                _ => {}
            }
        }
    }

    let mut paras = Vec::new();
    for idref in spine {
        let Some(href) = manifest.get(&idref) else { continue };
        if let Ok(html) = read_entry(&mut zip, &resolve(&base, href)) {
            paras.extend(html_to_paragraphs(&html));
        }
    }
    Ok(paras)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_hrefs() {
        assert_eq!(resolve("OEBPS", "Text/ch%201.xhtml#p3"), "OEBPS/Text/ch 1.xhtml");
        assert_eq!(resolve("OEBPS/content", "../Text/a.xhtml"), "OEBPS/Text/a.xhtml");
        assert_eq!(resolve("", "./a.html"), "a.html");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
