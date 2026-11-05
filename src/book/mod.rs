pub mod epub;
pub mod markup;
pub mod pdf;
pub mod txt;

use std::path::Path;

use anyhow::{bail, Result};

const EXTS: [&str; 3] = ["txt", "epub", "pdf"];

fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn is_book(path: &Path) -> bool {
    EXTS.contains(&ext(path).as_str())
}

/// Load a book as a list of paragraphs.
pub fn load(path: &Path) -> Result<Vec<String>> {
    let paras = match ext(path).as_str() {
        "epub" => epub::load(path)?,
        "pdf" => pdf::load(path)?,
        _ => txt::load(path)?,
    };
    if paras.is_empty() {
        bail!("no readable text");
    }
    Ok(paras)
}

pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF)
}

/// Collapse whitespace runs to one space, drop control and zero-width chars, and trim both
/// ends (full-width indent included). A run holding a line break next to a CJK char is dropped
/// entirely: that is a source line wrap inside a sentence, not a real space.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    let mut broke = false;
    for c in s.chars() {
        if c.is_whitespace() {
            pending = true;
            broke |= c == '\n' || c == '\r';
            continue;
        }
        if c.is_control() || matches!(c, '\u{feff}' | '\u{200b}' | '\u{200c}' | '\u{200d}') {
            continue;
        }
        if pending {
            if let Some(last) = out.chars().next_back() {
                if !(broke && (is_cjk(last) || is_cjk(c))) {
                    out.push(' ');
                }
            }
        }
        pending = false;
        broke = false;
        out.push(c);
    }
    out
}

fn ends_sentence(s: &str) -> bool {
    s.chars().next_back().is_some_and(|c| {
        matches!(c, '。' | '！' | '？' | '…' | '”' | '」' | '』' | '.' | '!' | '?' | '"' | '：' | ':')
    })
}

fn join(cur: &mut String, t: &str) {
    let (Some(last), Some(first)) = (cur.chars().next_back(), t.chars().next()) else {
        cur.push_str(t);
        return;
    };
    let before_hyphen = cur.chars().rev().nth(1);
    if last == '-' && first.is_ascii_lowercase() && before_hyphen.is_some_and(|c| c.is_alphabetic()) {
        cur.pop(); // re-join a word hyphenated across lines
    } else if !(is_cjk(last) || is_cjk(first)) {
        cur.push(' ');
    }
    cur.push_str(t);
}

fn flush(cur: &mut String, paras: &mut Vec<String>) {
    let p = normalize(cur);
    if !p.is_empty() {
        paras.push(p);
    }
    cur.clear();
}

/// Turn hard-wrapped lines (PDF text, Gutenberg-style txt) back into paragraphs. A paragraph
/// ends at a blank line, before an indented line, or after a short line that ends a sentence.
/// Bare page numbers are dropped.
pub fn reflow_lines(lines: &[&str]) -> Vec<String> {
    let mut lens: Vec<usize> = lines
        .iter()
        .map(|l| l.trim().chars().count())
        .filter(|&n| n > 0)
        .collect();
    lens.sort_unstable();
    let median = lens.get(lens.len() / 2).copied().unwrap_or(0);

    let mut paras = Vec::new();
    let mut cur = String::new();
    for raw in lines {
        let raw = raw.trim_end();
        let t = raw.trim_start();
        if t.is_empty() {
            flush(&mut cur, &mut paras);
            continue;
        }
        if t.len() <= 4 && t.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let lead = &raw[..raw.len() - t.len()];
        if lead.contains('\u{3000}') || lead.matches(' ').count() >= 2 {
            flush(&mut cur, &mut paras);
        }
        join(&mut cur, t);
        let n = t.chars().count();
        if median > 0 && n * 10 < median * 7 && ends_sentence(t) {
            flush(&mut cur, &mut paras);
        }
    }
    flush(&mut cur, &mut paras);
    paras
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_drops_space_between_cjk() {
        assert_eq!(normalize("\u{3000}\u{3000}他推开门，\n  屋里一片漆黑 "), "他推开门，屋里一片漆黑");
        assert_eq!(normalize(" hello \n world "), "hello world");
        assert_eq!(normalize("第一章 夜\t（EPUB  第1章）"), "第一章 夜 （EPUB 第1章）");
    }

    #[test]
    fn reflow_joins_wrapped_lines() {
        let lines = [
            "　　他推开门，屋里一片漆黑，什么也看不见，只能听见",
            "远处传来的风声。",
            "　　第二段开始了，这一行也很长很长很长很长很长很",
            "长。",
            "12",
            "",
            "The quick brown fox jumps over the lazy dog and keeps",
            "running far away into the night without look-",
            "ing back.",
        ];
        let paras = reflow_lines(&lines);
        assert_eq!(
            paras,
            [
                "他推开门，屋里一片漆黑，什么也看不见，只能听见远处传来的风声。",
                "第二段开始了，这一行也很长很长很长很长很长很长。",
                "The quick brown fox jumps over the lazy dog and keeps running far away into the night without looking back.",
            ]
        );
    }
}
