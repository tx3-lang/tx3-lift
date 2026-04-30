use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub chain: ChainConfig,
    pub storage: StorageConfig,
    #[serde(default)]
    pub watch: WatchConfig,
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChainConfig {
    pub endpoint: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub intersect: Intersect,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Intersect {
    Tag(IntersectTag),
    Point { slot: u64, hash: String },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IntersectTag {
    Tip,
}

impl Default for Intersect {
    fn default() -> Self {
        Self::Tag(IntersectTag::Tip)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StorageConfig {
    pub database_path: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WatchConfig {
    /// Match txs that consume or produce a UTxO at any of these bech32 addresses.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// Match txs that move (mint, burn, or transfer) any asset under this policy id (hex).
    #[serde(default)]
    pub moves_policy_id: Option<String>,
    /// Match txs that mint or burn any asset under this policy id (hex).
    #[serde(default)]
    pub mints_policy_id: Option<String>,
}

impl WatchConfig {
    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty()
            && self.moves_policy_id.is_none()
            && self.mints_policy_id.is_none()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SourceConfig {
    pub name: String,
    pub tii_path: PathBuf,
    pub profile: String,
}

pub fn load(path: impl AsRef<Path>) -> Result<Config> {
    let contents = std::fs::read_to_string(path.as_ref())?;
    let cfg: Config = toml::from_str(&contents)?;
    if cfg.sources.is_empty() {
        return Err(Error::Config(
            "at least one [[sources]] entry is required".to_string(),
        ));
    }
    Ok(cfg)
}
