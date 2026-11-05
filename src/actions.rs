//! herdr plugin actions. `open` / `close` / `toggle` / `follow` drive herdr's CLI; `next` /
//! `prev` / `boss` / `info` are forwarded to the running reader over its control socket.

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::Path,
    process::Command,
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::config::{self, PLUGIN_ID};

/// Rows herdr's pane frame takes on top of the usable area (measured on 0.9.3).
const FRAME_ROWS: u16 = 2;
const BIN_NAME: &str = "herdr-reader";
/// herdr clamps every split ratio to at most this (observed on 0.9.3).
const MAX_RATIO: f64 = 0.9;

pub fn run(action: &str) -> Result<()> {
    if let "next" | "prev" | "boss" | "info" | "quit" = action {
        return send(action);
    }
    // herdr runs every action and event hook on its own thread, so a tab switch fires
    // `tab.focused` and `workspace.focused` together; serialize the pane-moving actions.
    let _lock = Lock::take()?;
    match action {
        "open" => open(),
        "close" => close(),
        "toggle" => {
            if live_pane().is_some() {
                close()
            } else {
                open()
            }
        }
        "follow" => follow(),
        other => bail!("unknown action {other:?}"),
    }
}

/// An exclusive `flock` on a file in the state dir, released when dropped.
struct Lock(#[allow(dead_code)] File);

impl Lock {
    fn take() -> Result<Lock> {
        let dir = config::state_dir();
        fs::create_dir_all(&dir)?;
        let f = OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("actions.lock"))?;
        // SAFETY: flock on an fd we own; the lock goes away when the file is closed.
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
        Ok(Lock(f))
    }
}

fn send(cmd: &str) -> Result<()> {
    match UnixStream::connect(config::socket_path()) {
        Ok(mut s) => Ok(writeln!(s, "{cmd}")?),
        Err(_) => {
            println!("reader: not running");
            Ok(())
        }
    }
}

fn herdr(args: &[&str]) -> Result<Value> {
    let bin = env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
    let out = Command::new(&bin)
        .args(args)
        .output()
        .with_context(|| format!("running {bin}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        bail!("herdr {}: {}{}", args.join(" "), stdout.trim(), String::from_utf8_lossy(&out.stderr).trim());
    }
    serde_json::from_str(&stdout).with_context(|| format!("herdr {}: bad json", args[..2].join(" ")))
}

fn pane_file() -> std::path::PathBuf {
    config::state_dir().join("pane")
}

/// The pane id the last `open` created, if that pane still runs the reader.
fn live_pane() -> Option<String> {
    let id = fs::read_to_string(pane_file()).ok()?.trim().to_string();
    let info = herdr(&["pane", "process-info", "--pane", &id]).ok()?;
    let procs = info["result"]["process_info"]["foreground_processes"].as_array()?;
    // herdr 0.9.3 reports `argv` and `name` per process (no `argv0`); match either.
    procs
        .iter()
        .filter_map(|p| p["argv"][0].as_str().or_else(|| p["name"].as_str()))
        .any(|a| Path::new(a).file_name().is_some_and(|n| n == BIN_NAME))
        .then_some(id)
}

fn focused_pane() -> Result<String> {
    if let Ok(ctx) = env::var("HERDR_PLUGIN_CONTEXT_JSON") {
        let v: Value = serde_json::from_str(&ctx).unwrap_or(Value::Null);
        if let Some(id) = v["focused_pane_id"].as_str() {
            return Ok(id.to_string());
        }
    }
    env::var("HERDR_PANE_ID").map_err(|_| anyhow!("no focused pane in the action context"))
}

fn rect_height(layout: &Value, pane: &str) -> Option<f64> {
    layout["result"]["layout"]["panes"]
        .as_array()?
        .iter()
        .find(|p| p["pane_id"] == pane)?["rect"]["height"]
        .as_f64()
}

/// Squeeze the freshly split pane down to `rows` usable rows. Resizing `above` toward `down`
/// moves the split right under it, which is the one shared with the new pane, even when the
/// layout is nested (resizing the new pane itself can move a different split).
fn shrink(pane: &str, above: &str, rows: u16) -> Result<()> {
    let layout = herdr(&["pane", "layout", "--pane", pane])?;
    let (Some(h_new), Some(h_above)) = (rect_height(&layout, pane), rect_height(&layout, above)) else {
        return Ok(()); // not stacked as expected; leave herdr's default size
    };
    let need = f64::from(rows + FRAME_ROWS);
    if h_new == need {
        return Ok(());
    }
    let total = h_new + h_above;
    let ratio = h_above / total;
    // herdr rounds the upper share and caps split ratios at 0.9, so on a tall tab the
    // reader can't get below ~10% of the split; settle for that.
    let target = ((total - need) / total).clamp(0.5, MAX_RATIO);
    let delta = target - ratio;
    if delta.abs() < 0.5 / total {
        return Ok(());
    }
    let (dir, amount) = if delta > 0.0 { ("down", delta) } else { ("up", -delta) };
    herdr(&["pane", "resize", "--pane", above, "--direction", dir, "--amount", &format!("{amount:.4}")])?;
    Ok(())
}

fn open() -> Result<()> {
    if let Some(id) = live_pane() {
        println!("reader: already open in {id}");
        return Ok(());
    }
    let target = focused_pane()?;
    let res = herdr(&[
        "plugin", "pane", "open", "--plugin", PLUGIN_ID, "--entrypoint", "pane",
        "--placement", "split", "--direction", "down", "--target-pane", &target, "--no-focus",
    ])?;
    let id = res["result"]["plugin_pane"]["pane"]["pane_id"]
        .as_str()
        .context("plugin pane open: no pane id in reply")?
        .to_string();
    let dir = config::state_dir();
    fs::create_dir_all(&dir)?;
    fs::write(pane_file(), &id)?;
    if let Err(e) = shrink(&id, &target, config::load().pane_rows.max(1)) {
        println!("reader: could not resize: {e:#}");
    }
    println!("reader: opened {id}");
    Ok(())
}

fn rect(p: &Value) -> Option<(i64, i64, i64, i64)> {
    let r = &p["rect"];
    Some((r["x"].as_i64()?, r["y"].as_i64()?, r["width"].as_i64()?, r["height"].as_i64()?))
}

/// The pane whose bottom edge touches `pane`'s top edge, in a `panes` list of a layout.
fn pane_above(panes: &[Value], pane: &str) -> Option<String> {
    let (x, y, _, _) = rect(panes.iter().find(|p| p["pane_id"] == pane)?)?;
    panes
        .iter()
        .find(|p| {
            rect(p).is_some_and(|(px, py, pw, ph)| py + ph == y && px <= x && x < px + pw)
        })?["pane_id"]
        .as_str()
        .map(String::from)
}

/// Event hook for `tab.focused` / `workspace.focused`: keep the reader in whatever tab has
/// focus by moving it under the focused pane, and re-fit its height.
fn follow() -> Result<()> {
    let Some(id) = live_pane() else { return Ok(()) };
    let snap = herdr(&["api", "snapshot"])?;
    let s = &snap["result"]["snapshot"];
    let (Some(focused_tab), Some(focused_pane)) = (s["focused_tab_id"].as_str(), s["focused_pane_id"].as_str())
    else {
        return Ok(());
    };
    let layouts = s["layouts"].as_array().map(Vec::as_slice).unwrap_or_default();
    let tab_of = |pane: &str| {
        layouts.iter().find(|l| {
            l["panes"].as_array().is_some_and(|ps| ps.iter().any(|p| p["pane_id"] == pane))
        })
    };
    let rows = config::load().pane_rows.max(1);

    if tab_of(&id).is_some_and(|l| l["tab_id"] == focused_tab) {
        // Already here: just make sure it is still the right height.
        let panes = tab_of(&id).and_then(|l| l["panes"].as_array()).map(Vec::as_slice).unwrap_or_default();
        if let Some(above) = pane_above(panes, &id) {
            shrink(&id, &above, rows)?;
        }
        return Ok(());
    }
    if focused_pane == id || tab_of(focused_pane).is_some_and(|l| l["zoomed"] == true) {
        return Ok(());
    }
    let res = herdr(&[
        "pane", "move", &id, "--tab", focused_tab, "--target-pane", focused_pane,
        "--split", "down", "--ratio", "0.9", "--no-focus",
    ])?;
    // A move into another workspace re-numbers the pane, so take the id from the reply.
    let new_id = res["result"]["move_result"]["pane"]["pane_id"].as_str().unwrap_or(&id).to_string();
    fs::write(pane_file(), &new_id)?;
    shrink(&new_id, focused_pane, rows)?;
    println!("reader: followed to {new_id}");
    Ok(())
}

fn close() -> Result<()> {
    if let Some(id) = live_pane() {
        // Ask the reader to quit first so it saves progress, then close the pane.
        let _ = send("quit");
        std::thread::sleep(std::time::Duration::from_millis(150));
        let _ = herdr(&["pane", "close", &id]);
        println!("reader: closed {id}");
    }
    let _ = fs::remove_file(pane_file());
    Ok(())
}
