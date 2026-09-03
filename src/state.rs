//! Per-machine state: `state.toml` in the platform state directory.
//!
//! Milestone 2 only reads it (tags, repo path). Writing, per-module setup
//! versions, journal and trash arrive with the apply stage.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const STATE_FILE: &str = "state.toml";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub machine: Machine,
    pub modules: BTreeMap<String, ModuleState>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Machine {
    pub tags: Vec<String>,
    pub repo: Option<PathBuf>,
    pub identity: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleState {
    pub setup_version: Option<u32>,
    pub setup_at: Option<String>,
    pub last_push: Option<String>,
    pub last_pull: Option<String>,
}

/// `QD_STATE` overrides; otherwise the platform state dir, falling back to the
/// data dir on platforms without one (macOS).
pub fn state_dir() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("QD_STATE") {
        return Ok(PathBuf::from(p));
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|d| d.join("qd"))
        .context("cannot determine a state directory; set QD_STATE")
}

impl State {
    pub fn load_from(dir: &Path) -> Result<State> {
        let file = dir.join(STATE_FILE);
        if !file.exists() {
            return Ok(State::default());
        }
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))
    }

    pub fn load() -> Result<State> {
        State::load_from(&state_dir()?)
    }

    pub fn save_to(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let file = dir.join(STATE_FILE);
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&file, text).with_context(|| format!("writing {}", file.display()))
    }
}
