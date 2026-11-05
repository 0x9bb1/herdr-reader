use std::{collections::HashMap, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;

/// A reading position that survives re-wrapping: paragraph index plus char offset inside it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pos {
    pub para: usize,
    pub offset: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub last: Option<String>,
    #[serde(default)]
    pub books: HashMap<String, Pos>,
}

fn path() -> PathBuf {
    config::state_dir().join("progress.json")
}

impl Store {
    pub fn load() -> Store {
        fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Write via a temp file + rename so a crash never leaves a half-written file.
    pub fn save(&self) -> io::Result<()> {
        let p = path();
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = p.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(tmp, p)
    }
}
