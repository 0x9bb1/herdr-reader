use std::{fs, path::Path};

use anyhow::Result;

use super::{normalize, reflow_lines};

pub fn load(path: &Path) -> Result<Vec<String>> {
    Ok(paragraphs(&decode(&fs::read(path)?)))
}

/// UTF-8 (BOM optional); UTF-16 with a BOM; otherwise GB18030, the usual legacy Chinese encoding.
fn decode(bytes: &[u8]) -> String {
    if let Some((enc, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

/// Chinese web-novel txt has one paragraph per line. English txt (Gutenberg style) is
/// hard-wrapped at ~70 columns with blank lines between paragraphs, so it gets re-flowed.
fn paragraphs(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let nonblank: Vec<&str> = lines.iter().copied().filter(|l| !l.trim().is_empty()).collect();
    let blank = lines.len() - nonblank.len();
    let (mut chars, mut ascii) = (0usize, 0usize);
    for l in &nonblank {
        for c in l.chars() {
            chars += 1;
            ascii += c.is_ascii() as usize;
        }
    }
    let avg = if nonblank.is_empty() { 0 } else { chars / nonblank.len() };
    let hard_wrapped = ascii * 10 > chars * 8 && blank * 8 >= nonblank.len() && (30..100).contains(&avg);
    if hard_wrapped {
        reflow_lines(&lines)
    } else {
        nonblank.iter().map(|l| normalize(l)).filter(|p| !p.is_empty()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_gbk() {
        let (bytes, _, _) = encoding_rs::GBK.encode("第一章 开始");
        assert_eq!(decode(&bytes), "第一章 开始");
        assert_eq!(decode("\u{feff}你好".as_bytes()), "你好");
    }

    #[test]
    fn one_paragraph_per_line_for_cjk() {
        let p = paragraphs("第一章\n\n　　他推开门。\r\n　　屋里一片漆黑。\n");
        assert_eq!(p, ["第一章", "他推开门。", "屋里一片漆黑。"]);
    }

    #[test]
    fn reflows_hard_wrapped_english() {
        let text = "It was a bright cold day in April, and the clocks were\nstriking thirteen. Winston Smith hurried along quickly now.\n\nThe hallway smelt of boiled cabbage and old rag mats. At\none end of it a coloured poster was tacked to the wall.\n";
        let p = paragraphs(text);
        assert_eq!(p.len(), 2);
        assert!(p[0].starts_with("It was a bright cold day in April, and the clocks were striking"));
    }
}
