use std::{
    io::{self, Write},
    panic,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{anyhow, bail, Result};

use super::reflow_lines;

pub fn load(path: &Path) -> Result<Vec<String>> {
    let text = match pdftotext(path) {
        Some(t) => t,
        None => extract_builtin(path)?,
    };
    let lines: Vec<&str> = text.lines().collect();
    let paras = reflow_lines(&lines);
    if paras.is_empty() {
        bail!("no text layer (scanned PDF?)");
    }
    Ok(paras)
}

/// poppler's `pdftotext` handles CJK fonts far better than the pure-Rust parser; use it if present.
fn pdftotext(path: &Path) -> Option<String> {
    let out = Command::new("pdftotext")
        .args(["-enc", "UTF-8"])
        .arg(path)
        .arg("-")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Points fds 1 and 2 at /dev/null while alive: the PDF parser may print diagnostics,
/// which would scribble over the TUI.
struct Silence {
    saved: Option<(i32, i32)>,
}

impl Silence {
    fn new() -> Silence {
        let _ = io::stdout().flush();
        // SAFETY: plain fd juggling; every fd opened here is closed in Drop.
        unsafe {
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
            if null < 0 {
                return Silence { saved: None };
            }
            let (out, err) = (libc::dup(1), libc::dup(2));
            libc::dup2(null, 1);
            libc::dup2(null, 2);
            libc::close(null);
            Silence { saved: Some((out, err)) }
        }
    }
}

impl Drop for Silence {
    fn drop(&mut self) {
        let _ = io::stdout().flush();
        if let Some((out, err)) = self.saved {
            // SAFETY: restores the fds saved in `new`.
            unsafe {
                libc::dup2(out, 1);
                libc::dup2(err, 2);
                libc::close(out);
                libc::close(err);
            }
        }
    }
}

fn extract_builtin(path: &Path) -> Result<String> {
    let _quiet = Silence::new();
    // The parser can panic on odd files; keep that from tearing down the TUI.
    let hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let res = panic::catch_unwind(|| pdf_extract::extract_text(path));
    panic::set_hook(hook);
    match res {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(anyhow!("pdf: {e}")),
        Err(_) => Err(anyhow!("pdf parser crashed on this file")),
    }
}
