//! herdr plugin actions. `open` / `close` / `toggle` / `follow` drive herdr's CLI; `next` /
//! `prev` / `boss` / `info` are forwarded to the running reader over its control socket.

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::{Path, PathBuf},
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
/// The label herdr gives our panes: their `title` in herdr-plugin.toml.
const PANE_LABEL: &str = "logs";

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
        "restore" => restore(Revive::OldSlot).map(drop),
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

fn holders_file() -> std::path::PathBuf {
    config::state_dir().join("placeholders")
}

/// Whether `pane`'s foreground process is this binary, run as a placeholder or as the reader.
/// Pane ids get reused, so a stored id alone is never trusted.
fn runs_us(pane: &str, placeholder: bool) -> bool {
    let Ok(info) = herdr(&["pane", "process-info", "--pane", pane]) else { return false };
    let Some(procs) = info["result"]["process_info"]["foreground_processes"].as_array() else {
        return false;
    };
    procs.iter().any(|p| {
        let holder = p["argv"].as_array().is_some_and(|a| a.iter().any(|x| x == "--placeholder"));
        is_us(p) && holder == placeholder
    })
}

/// Whether a `pane process-info` process entry is this binary.
/// herdr 0.9.3 reports `argv` and `name` per process (no `argv0`); match either.
fn is_us(p: &Value) -> bool {
    p["argv"][0]
        .as_str()
        .or_else(|| p["name"].as_str())
        .is_some_and(|a| Path::new(a).file_name().is_some_and(|n| n == BIN_NAME))
}

/// The folder herdr runs our commands in.
fn plugin_root() -> Option<PathBuf> {
    env::var_os("HERDR_PLUGIN_ROOT").map(PathBuf::from).or_else(|| env::current_dir().ok())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b || fs::canonicalize(a).is_ok_and(|a| fs::canonicalize(b).is_ok_and(|b| a == b))
}

/// Whether nothing but the shell runs in `pane`, and that shell is not this binary.
fn idle_shell(pane: &str) -> bool {
    let Ok(info) = herdr(&["pane", "process-info", "--pane", pane]) else { return false };
    let info = &info["result"]["process_info"];
    let procs = info["foreground_processes"].as_array().map(Vec::as_slice).unwrap_or_default();
    info["shell_pid"].is_u64()
        && info["foreground_process_group_id"] == info["shell_pid"]
        && !procs.iter().any(is_us)
}

/// Our panes as a herdr server restart (an upgrade, say) leaves them: herdr saves a plugin
/// pane as just its folder and label, so the reader and every placeholder come back as
/// plain shells, labeled "logs", in the plugin folder, under their old pane ids.
/// Returns (pane id, tab id) for each one nobody has started using.
fn stale_panes() -> Result<Vec<(String, String)>> {
    let Some(root) = plugin_root() else { return Ok(Vec::new()) };
    let list = herdr(&["pane", "list"])?;
    let panes = list["result"]["panes"].as_array().map(Vec::as_slice).unwrap_or_default();
    Ok(panes
        .iter()
        .filter(|p| p["label"] == PANE_LABEL && p["cwd"].as_str().is_some_and(|c| same_dir(Path::new(c), &root)))
        .filter_map(|p| Some((p["pane_id"].as_str()?.to_string(), p["tab_id"].as_str()?.to_string())))
        .filter(|(id, _)| idle_shell(id))
        .collect())
}

/// Which of the shells `restore` refills with the reader; the rest get placeholders.
enum Revive<'a> {
    /// The reader's own old slot (startup: it was open when herdr went down).
    OldSlot,
    /// The one right under this pane (`open`).
    Under(&'a str),
}

/// Startup hook (and the first step of `open`): put our panes back after a herdr restart
/// turned them into shells. Each one is refilled in place, by splitting it sideways and
/// closing it, so no pane above changes height. Returns whether the reader was reopened.
fn restore(revive: Revive) -> Result<bool> {
    let stale = stale_panes()?;
    if stale.is_empty() {
        return Ok(false);
    }
    let reader = match revive {
        Revive::OldSlot => stored_pane().map(|(id, _)| id),
        Revive::Under(target) => {
            let s = snapshot()?;
            let layouts = s["layouts"].as_array().map(Vec::as_slice).unwrap_or_default();
            let panes = panes_of(tab_of(layouts, target));
            stale.iter().map(|(p, _)| p).find(|p| pane_above(panes, p).as_deref() == Some(target)).cloned()
        }
    };
    let mut reopened = false;
    let mut holders: Vec<String> =
        stored_holders().into_iter().filter(|h| !stale.iter().any(|(p, _)| p == h)).collect();
    for (pane, tab) in &stale {
        let is_reader = reader.as_deref() == Some(pane.as_str());
        let entrypoint = if is_reader { "pane" } else { "placeholder" };
        let id = match open_split(entrypoint, pane, "right") {
            Ok(id) => id,
            Err(e) => {
                println!("reader: could not refill {pane}: {e:#}");
                continue;
            }
        };
        if let Err(e) = herdr(&["pane", "close", pane]) {
            println!("reader: could not close {pane}: {e:#}");
        }
        if is_reader {
            remember(&id, tab)?;
            reopened = true;
            println!("reader: reopened in {id}");
        } else {
            holders.push(id);
        }
    }
    save_holders(&holders)?;
    println!("reader: refilled {} pane(s) left as shells", stale.len());
    Ok(reopened)
}

/// The reader's pane id and the tab it was last put in, as `open` / `follow` left them.
/// Files written before the tab was recorded hold the pane id alone.
fn stored_pane() -> Option<(String, Option<String>)> {
    let s = fs::read_to_string(config::pane_file()).ok()?;
    let mut lines = s.lines().map(str::trim);
    let pane = lines.next().filter(|p| !p.is_empty())?.to_string();
    let tab = lines.next().filter(|t| !t.is_empty()).map(String::from);
    Some((pane, tab))
}

fn remember(pane: &str, tab: &str) -> Result<()> {
    fs::create_dir_all(config::state_dir())?;
    fs::write(config::pane_file(), format!("{pane}\n{tab}\n"))?;
    Ok(())
}

/// The pane id the last `open` created, if that pane still runs the reader.
fn live_pane() -> Option<String> {
    let (id, _) = stored_pane()?;
    runs_us(&id, false).then_some(id)
}

fn stored_holders() -> Vec<String> {
    fs::read_to_string(holders_file())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(String::from)
        .collect()
}

fn save_holders(ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        let _ = fs::remove_file(holders_file());
    } else {
        fs::write(holders_file(), ids.join("\n") + "\n")?;
    }
    Ok(())
}

fn snapshot() -> Result<Value> {
    Ok(herdr(&["api", "snapshot"])?["result"]["snapshot"].take())
}

/// The layout (one per tab) holding `pane`.
fn tab_of<'a>(layouts: &'a [Value], pane: &str) -> Option<&'a Value> {
    layouts
        .iter()
        .find(|l| l["panes"].as_array().is_some_and(|ps| ps.iter().any(|p| p["pane_id"] == pane)))
}

fn panes_of(layout: Option<&Value>) -> &[Value] {
    layout.and_then(|l| l["panes"].as_array()).map(Vec::as_slice).unwrap_or_default()
}

/// Take out of `holders` the placeholder sitting right under `pane`, if there is one.
fn take_holder_under(layouts: &[Value], holders: &mut Vec<String>, pane: &str) -> Option<String> {
    let panes = panes_of(tab_of(layouts, pane));
    let i = holders.iter().position(|h| {
        panes.iter().any(|p| p["pane_id"] == h.as_str()) && pane_above(panes, h).as_deref() == Some(pane)
    })?;
    let h = holders.remove(i);
    runs_us(&h, true).then_some(h)
}

/// Open the reader or a placeholder (`entrypoint`) as a split of `target`, without focus.
fn open_split(entrypoint: &str, target: &str, direction: &str) -> Result<String> {
    let res = herdr(&[
        "plugin", "pane", "open", "--plugin", PLUGIN_ID, "--entrypoint", entrypoint,
        "--placement", "split", "--direction", direction, "--target-pane", target, "--no-focus",
    ])?;
    Ok(res["result"]["plugin_pane"]["pane"]["pane_id"]
        .as_str()
        .context("plugin pane open: no pane id in reply")?
        .to_string())
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
/// Returns whether a resize was issued.
fn shrink(pane: &str, above: &str, rows: u16) -> Result<bool> {
    let layout = herdr(&["pane", "layout", "--pane", pane])?;
    let (Some(h_new), Some(h_above)) = (rect_height(&layout, pane), rect_height(&layout, above)) else {
        return Ok(false); // not stacked as expected; leave herdr's default size
    };
    let need = f64::from(rows + FRAME_ROWS);
    if h_new == need {
        return Ok(false);
    }
    let total = h_new + h_above;
    let ratio = h_above / total;
    // herdr rounds the upper share and caps split ratios at 0.9, so on a tall tab the
    // reader can't get below ~10% of the split; settle for that.
    let target = ((total - need) / total).clamp(0.5, MAX_RATIO);
    let delta = target - ratio;
    if delta.abs() < 0.5 / total {
        return Ok(false);
    }
    let (dir, amount) = if delta > 0.0 { ("down", delta) } else { ("up", -delta) };
    herdr(&["pane", "resize", "--pane", above, "--direction", dir, "--amount", &format!("{amount:.4}")])?;
    Ok(true)
}

/// Delay before `refocus`, so the pane above has repainted at its new size first.
const REFOCUS_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// Shrinking the pane in the tab on screen leaves it drawn at the old size and scrolled
/// off the bottom until it is clicked (seen with Claude Code). Re-focusing it does what the
/// click does. Skipped if focus moved on meanwhile, so we never pull the user back.
fn refocus(reader: &str, above: &str) {
    std::thread::sleep(REFOCUS_DELAY);
    let still_there = snapshot().is_ok_and(|s| s["focused_pane_id"] == above);
    if still_there {
        let _ = herdr(&["pane", "focus", "--pane", reader, "--direction", "up"]);
    }
}

fn open() -> Result<()> {
    if let Some(id) = live_pane() {
        println!("reader: already open in {id}");
        return Ok(());
    }
    let target = focused_pane()?;
    // Shells a herdr restart left in our slots (if the startup hook missed them) get refilled
    // first: the one under `target` with the reader, so it does not land between the two.
    match restore(Revive::Under(&target)) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) => println!("reader: could not refill old panes: {e:#}"),
    }
    open_under(&target)
}

/// Open the reader under `target`, taking over the placeholder slot there if there is one.
fn open_under(target: &str) -> Result<()> {
    // Placeholders outlive a reader that quit on its own (`q`); reuse the one under `target`.
    let s = snapshot()?;
    let layouts = s["layouts"].as_array().map(Vec::as_slice).unwrap_or_default();
    let tab = tab_of(layouts, target).and_then(|l| l["tab_id"].as_str()).unwrap_or_default().to_string();
    let mut holders: Vec<String> =
        stored_holders().into_iter().filter(|h| tab_of(layouts, h).is_some()).collect();
    let id = match take_holder_under(layouts, &mut holders, target) {
        Some(h) => {
            let id = open_split("pane", &h, "right")?;
            herdr(&["pane", "close", &h])?;
            id
        }
        None => open_split("pane", target, "down")?,
    };
    remember(&id, &tab)?;
    save_holders(&holders)?;
    if let Err(e) = shrink(&id, target, config::load().pane_rows.max(1)) {
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

/// Move `pane` into `tab` next to `target`, splitting it `split` with `ratio`; returns the
/// pane's id afterwards (a move into another workspace re-numbers it).
fn move_to(pane: &str, tab: &str, target: &str, split: &str, ratio: &str) -> Result<String> {
    let res = herdr(&[
        "pane", "move", pane, "--tab", tab, "--target-pane", target, "--split", split, "--ratio", ratio,
        "--no-focus",
    ])?;
    Ok(res["result"]["move_result"]["pane"]["pane_id"].as_str().unwrap_or(pane).to_string())
}

/// Event hook for `tab.focused` / `workspace.focused`: keep the reader in whatever tab has
/// focus by moving it under the focused pane, and re-fit its height.
///
/// Squeezing the pane above while its tab is on screen makes it redraw (and Claude Code
/// draws it wrong until clicked), so the reader leaves a same-sized placeholder behind in
/// each tab it moves out of. Coming back, it slides in sideways next to that placeholder and
/// the placeholder closes: no pane above it ever changes height. Only the first visit to a
/// tab has to split and squeeze.
fn follow() -> Result<()> {
    let Some(id) = live_pane() else { return revive() };
    let s = snapshot()?;
    let (Some(focused_tab), Some(focused_pane)) = (s["focused_tab_id"].as_str(), s["focused_pane_id"].as_str())
    else {
        return Ok(());
    };
    let layouts = s["layouts"].as_array().map(Vec::as_slice).unwrap_or_default();
    let rows = config::load().pane_rows.max(1);
    let here = tab_of(layouts, &id);

    if here.is_some_and(|l| l["tab_id"] == focused_tab) {
        // Already here: just make sure it is still the right height.
        if let Some(above) = pane_above(panes_of(here), &id) {
            if shrink(&id, &above, rows)? {
                refocus(&id, &above);
            }
        }
        return Ok(());
    }
    if focused_pane == id || tab_of(layouts, focused_pane).is_some_and(|l| l["zoomed"] == true) {
        return Ok(());
    }

    let mut holders: Vec<String> =
        stored_holders().into_iter().filter(|h| tab_of(layouts, h).is_some()).collect();
    // Split the reader sideways so the placeholder inherits its whole slot once it leaves.
    if pane_above(panes_of(here), &id).is_some() {
        match open_split("placeholder", &id, "right") {
            Ok(h) => holders.push(h),
            Err(e) => println!("reader: no placeholder left behind: {e:#}"),
        }
    }
    let slot = take_holder_under(layouts, &mut holders, focused_pane);
    save_holders(&holders)?;

    let new_id = match slot {
        Some(h) => {
            let new_id = move_to(&id, focused_tab, &h, "right", "0.5")?;
            herdr(&["pane", "close", &h])?;
            // Same height as before unless `pane_rows` changed meanwhile.
            if shrink(&new_id, focused_pane, rows)? {
                refocus(&new_id, focused_pane);
            }
            new_id
        }
        None => {
            let new_id = move_to(&id, focused_tab, focused_pane, "down", "0.9")?;
            shrink(&new_id, focused_pane, rows)?;
            refocus(&new_id, focused_pane);
            new_id
        }
    };
    remember(&new_id, focused_tab)?;
    println!("reader: followed to {new_id}");
    Ok(())
}

/// Closing a tab (or workspace) kills every pane in it, the reader included, and the tab you
/// land in is left showing its placeholder's fake log. So when the reader died together with
/// the tab it was in, reopen it under the focused pane, in the placeholder slot there.
/// A reader that quit on purpose (`q`, `close`) has dropped the pane file, and one whose pane
/// alone was closed still has its tab: both stay closed.
fn revive() -> Result<()> {
    let Some((_, Some(tab))) = stored_pane() else { return Ok(()) };
    let s = snapshot()?;
    let layouts = s["layouts"].as_array().map(Vec::as_slice).unwrap_or_default();
    if layouts.iter().any(|l| l["tab_id"] == tab.as_str()) {
        return Ok(());
    }
    let Some(focused) = s["focused_pane_id"].as_str() else { return Ok(()) };
    if tab_of(layouts, focused).is_some_and(|l| l["zoomed"] == true) {
        return Ok(());
    }
    open_under(focused)?;
    println!("reader: reopened after tab {tab} closed");
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
    for h in stored_holders() {
        if runs_us(&h, true) {
            let _ = herdr(&["pane", "close", &h]);
        }
    }
    // Shells a herdr restart left in our old slots.
    for (pane, _) in stale_panes().unwrap_or_default() {
        let _ = herdr(&["pane", "close", &pane]);
    }
    let _ = fs::remove_file(config::pane_file());
    let _ = fs::remove_file(holders_file());
    Ok(())
}
