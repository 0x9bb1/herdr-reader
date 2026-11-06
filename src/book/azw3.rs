use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::UNIX_EPOCH,
};

use anyhow::{bail, Context, Result};

use crate::config;

/// Kindle AZW3 (KF8) has no parser here: calibre's `ebook-convert` turns it into an EPUB, which
/// is cached under the state dir (conversion takes seconds) and read by the EPUB loader.
pub fn load(path: &Path) -> Result<Vec<String>> {
    let epub = cache_path(path)?;
    if !epub.exists() {
        convert(path, &epub)?;
    }
    super::epub::load(&epub)
}

/// One cache file per source path + size + mtime, so a replaced book is converted again.
fn cache_path(path: &Path) -> Result<PathBuf> {
    let meta = fs::metadata(path)?;
    let mtime = meta.modified()?.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut h = DefaultHasher::new();
    (path, meta.len(), mtime).hash(&mut h);
    Ok(config::state_dir().join("cache").join(format!("{:016x}.epub", h.finish())))
}

/// `ebook-convert` on PATH, else where calibre's own installer puts it.
fn ebook_convert() -> Option<PathBuf> {
    let on_path = Command::new("ebook-convert")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok();
    if on_path {
        return Some(PathBuf::from("ebook-convert"));
    }
    ["/opt/calibre/ebook-convert", "/Applications/calibre.app/Contents/MacOS/ebook-convert"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

fn convert(src: &Path, dst: &Path) -> Result<()> {
    let Some(tool) = ebook_convert() else {
        bail!("reading .azw3 needs calibre's ebook-convert (sudo apt install calibre)");
    };
    let dir = dst.parent().context("bad cache path")?;
    fs::create_dir_all(dir)?;
    // ebook-convert picks the output format from the extension, so the temp name keeps `.epub`.
    let tmp = dst.with_extension("part.epub");
    let out = Command::new(tool)
        .arg(src)
        .arg(&tmp)
        .stdin(Stdio::null())
        .output()
        .context("failed to run ebook-convert")?;
    if !out.status.success() {
        let _ = fs::remove_file(&tmp);
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
        if err.contains("DRM") {
            bail!("book is DRM-protected");
        }
        bail!("ebook-convert failed: {}", last.trim());
    }
    fs::rename(&tmp, dst)?;
    Ok(())
}
