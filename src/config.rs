use std::{env, fs, path::PathBuf};

use serde::Deserialize;

pub const PLUGIN_ID: &str = "luna7.reader";

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// `$HERDR_PLUGIN_STATE_DIR`, or the same path herdr would give it, so a hand-launched pane
/// and the plugin actions agree on where the socket and progress live.
pub fn state_dir() -> PathBuf {
    env::var_os("HERDR_PLUGIN_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state/herdr/plugins").join(PLUGIN_ID))
}

pub fn config_dir() -> PathBuf {
    env::var_os("HERDR_PLUGIN_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config/herdr/plugins/config").join(PLUGIN_ID))
}

pub fn socket_path() -> PathBuf {
    state_dir().join("reader.sock")
}

pub fn expand(s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if s == "~" => home(),
        None => PathBuf::from(s),
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Folder scanned (two levels deep) for .txt / .epub / .pdf when pressing `o`.
    pub library: String,
    /// Fake module names put in front of each line, e.g. `[worker]`.
    pub modules: Vec<String>,
    /// Height of the pane the `open` action creates, in rows.
    pub pane_rows: u16,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            library: "~/books".into(),
            modules: ["worker", "sync", "http", "db", "sched", "cache"]
                .map(String::from)
                .to_vec(),
            pane_rows: 1,
        }
    }
}

pub fn load() -> Config {
    let path = config_dir().join("config.toml");
    fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}
