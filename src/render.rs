use std::{
    io::{self, Write},
    time::{SystemTime, UNIX_EPOCH},
};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use unicode_width::UnicodeWidthChar;

/// One wrapped display line of the book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub para: usize,
    /// Char offset of this line's first char inside its paragraph.
    pub start: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Debug,
    Warn,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Warn => "WARN ",
        }
    }

    fn color(self) -> Color {
        match self {
            Level::Info => Color::DarkGreen,
            Level::Debug => Color::DarkBlue,
            Level::Warn => Color::DarkYellow,
        }
    }
}

/// One log-looking row: `HH:MM:SS LEVEL [module] text`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub time: i64,
    pub level: Level,
    pub module: String,
    pub text: String,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Local wall-clock `HH:MM:SS`.
pub fn hms(t: i64) -> String {
    // SAFETY: localtime_r only writes into the `tm` we own.
    unsafe {
        let tt = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&tt, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

/// Columns taken by `HH:MM:SS LEVEL [module] `.
pub fn prefix_width(module_w: usize) -> usize {
    8 + 1 + 5 + 1 + module_w + 2 + 1
}

fn char_w(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// CJK closing punctuation that should never begin a line.
fn no_line_start(c: char) -> bool {
    matches!(c, '，' | '。' | '、' | '；' | '：' | '！' | '？' | '）' | '》' | '」' | '』' | '”' | '’' | '…' | '—')
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '\'' || c == '-'
}

/// Wrap one paragraph into `(start char offset, text)` lines no wider than `width` columns.
/// Latin words are not split when a space earlier in the line allows a cleaner break.
pub fn wrap_para(text: &str, width: usize) -> Vec<(usize, String)> {
    let width = width.max(2);
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if !out.is_empty() {
            while i < n && chars[i] == ' ' {
                i += 1;
            }
            if i >= n {
                break;
            }
        }
        let mut w = 0;
        let mut j = i;
        while j < n {
            let cw = char_w(chars[j]);
            if w + cw > width {
                break;
            }
            w += cw;
            j += 1;
        }
        if j == i {
            j = i + 1; // a single char wider than the line
        }
        if j < n && j - i > 1 && no_line_start(chars[j]) {
            j -= 1; // carry one char down so the punctuation doesn't open the next line
        } else if j < n && is_word(chars[j]) && is_word(chars[j - 1]) {
            if let Some(k) = (i + 1..j).rev().find(|&k| chars[k] == ' ') {
                j = k + 1;
            }
        }
        let s: String = chars[i..j].iter().collect();
        out.push((i, s.trim_end().to_string()));
        i = j;
    }
    out
}

pub fn wrap_book(paras: &[String], width: usize) -> Vec<Line> {
    let mut out = Vec::new();
    for (para, p) in paras.iter().enumerate() {
        for (start, text) in wrap_para(p, width) {
            out.push(Line { para, start, text });
        }
    }
    out
}

/// Index of the line containing char `offset` of paragraph `para`.
pub fn find_line(lines: &[Line], para: usize, offset: usize) -> usize {
    lines
        .partition_point(|l| (l.para, l.start) <= (para, offset))
        .saturating_sub(1)
}

fn fit(s: &str, w: usize) -> String {
    let mut acc = 0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = char_w(c);
        if acc + cw > w {
            break;
        }
        acc += cw;
        out.push(c);
    }
    out
}

/// Paint `entries` from the top row down; leftover rows are blanked.
pub fn draw(
    out: &mut impl Write,
    entries: &[Entry],
    module_w: usize,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let text_w = (cols as usize).saturating_sub(prefix_width(module_w) + 1);
    queue!(out, BeginSynchronizedUpdate)?;
    for row in 0..rows {
        queue!(out, MoveTo(0, row))?;
        match entries.get(row as usize) {
            Some(e) => {
                let module = format!("[{:<w$}]", e.module, w = module_w);
                queue!(
                    out,
                    SetForegroundColor(Color::DarkGrey),
                    Print(hms(e.time)),
                    Print(" "),
                    SetForegroundColor(e.level.color()),
                    Print(e.level.label()),
                    Print(" "),
                    SetForegroundColor(Color::DarkCyan),
                    Print(module),
                    Print(" "),
                    SetForegroundColor(Color::DarkGrey),
                    Print(fit(&e.text, text_w)),
                    ResetColor,
                    Clear(ClearType::UntilNewLine),
                )?;
            }
            None => queue!(out, Clear(ClearType::CurrentLine))?,
        }
    }
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn wraps_cjk_by_display_width() {
        let lines = wrap_para("他推开门屋里一片漆黑", 8);
        let texts: Vec<_> = lines.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(texts, ["他推开门", "屋里一片", "漆黑"]);
        assert_eq!(lines.iter().map(|l| l.0).collect::<Vec<_>>(), [0, 4, 8]);
    }

    #[test]
    fn punctuation_never_starts_a_line() {
        let lines = wrap_para("他推开门，屋里一片漆黑。", 8);
        let texts: Vec<_> = lines.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(texts, ["他推开", "门，屋里", "一片漆", "黑。"]);
    }

    #[test]
    fn keeps_latin_words_whole() {
        let lines = wrap_para("hello wonderful world", 12);
        let texts: Vec<_> = lines.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(texts, ["hello", "wonderful", "world"]);
        for (_, s) in &lines {
            assert!(s.width() <= 12);
        }
    }

    #[test]
    fn mixed_text_never_overflows() {
        for w in 3..30 {
            for (_, s) in wrap_para("Rust 是一门 systems 编程语言，安全又快。", w) {
                assert!(s.width() <= w.max(2), "{s:?} wider than {w}");
            }
        }
    }

    #[test]
    fn position_survives_rewrap() {
        let paras = vec!["一二三四五六七八九十".to_string(), "abc def ghi".to_string()];
        let narrow = wrap_book(&paras, 4);
        let idx = find_line(&narrow, 0, 6);
        assert_eq!(narrow[idx].start, 6);
        let wide = wrap_book(&paras, 10);
        let idx = find_line(&wide, 0, 6);
        assert_eq!((wide[idx].para, wide[idx].start), (0, 5));
        assert_eq!(find_line(&wide, 9, 0), wide.len() - 1);
    }
}
