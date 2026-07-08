use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub upstream: UpstreamConfig,
    pub storage: StorageConfig,
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
    #[serde(default)]
    pub matching: MatchingConfig,
}

/// Controls which match candidates the tracker retains per transaction.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MatchingConfig {
    #[serde(default)]
    pub mode: MatchMode,
}

/// Candidate-selection strategy used by the matcher loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// Keep all match candidates (default).
    #[default]
    All,
    /// Keep only the highest-ranked candidate per transaction.
    Best,
}

/// Where the tracker pulls chain data from.
///
/// Holds the gRPC endpoint, optional auth, the resume point, and an optional
/// `filter` block that narrows what the upstream forwards to us (server-side
/// pre-filter via `WatchTx`'s `TxPredicate`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpstreamConfig {
    pub endpoint: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub intersect: Intersect,
    #[serde(default)]
    pub filter: UpstreamFilter,
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

/// Server-side pre-filter applied to the WatchTx stream. Empty = forward
/// every tx; populated = forward only txs that match at least one alternative.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct UpstreamFilter {
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

impl UpstreamFilter {
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

/// Resolve the API key from two possible sources.
///
/// Explicit TOML value (`toml`) wins when present; otherwise the environment
/// variable value (`env`) is used. Returns `None` when both are absent.
/// This is a pure function — callers read `std::env::var` and pass the result
/// in, keeping this function deterministic and testable.
pub(crate) fn resolve_api_key(toml: Option<String>, env: Option<String>) -> Option<String> {
    toml.or(env)
}

pub fn load(path: impl AsRef<Path>) -> Result<Config> {
    let contents = std::fs::read_to_string(path.as_ref())?;
    let mut cfg: Config = toml::from_str(&contents)?;
    if cfg.sources.is_empty() {
        return Err(Error::Config(
            "at least one [[sources]] entry is required".to_string(),
        ));
    }
    // Both TOML and env sources ignore empty strings so neither can shadow the other with "".
    cfg.upstream.api_key = resolve_api_key(
        cfg.upstream.api_key.take().filter(|s| !s.is_empty()),
        std::env::var("DMTR_API_KEY").ok().filter(|s| !s.is_empty()),
    );
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    // Safety note: the `resolve_api_key` tests below are env-safe because they
    // call the pure function directly without touching `std::env`. Any future
    // test that exercises `config::load` with `DMTR_API_KEY` set must serialize
    // env access (e.g. via a mutex) to avoid races with parallel test threads.
    use super::*;

    const MINIMAL_TOML: &str = r#"
[upstream]
endpoint = "http://localhost:50051"

[storage]
database_path = "/tmp/tracker.db"

[[sources]]
name = "test"
tii_path = "/tmp/test.tii"
profile = "mainnet"
"#;

    #[test]
    fn resolve_api_key_uses_env_when_toml_absent() {
        let result = resolve_api_key(None, Some("env-key".to_string()));
        assert_eq!(result, Some("env-key".to_string()));
    }

    #[test]
    fn resolve_api_key_toml_wins_when_both_present() {
        let result = resolve_api_key(Some("toml-key".to_string()), Some("env-key".to_string()));
        assert_eq!(result, Some("toml-key".to_string()));
    }

    #[test]
    fn resolve_api_key_none_when_both_absent() {
        let result = resolve_api_key(None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_api_key_empty_toml_falls_back_to_env() {
        // An explicit api_key = "" in TOML must not shadow a real env value.
        let result = resolve_api_key(
            Some("".to_string()).filter(|s| !s.is_empty()),
            Some("env-key".to_string()),
        );
        assert_eq!(result, Some("env-key".to_string()));
    }

    #[test]
    fn matching_defaults_to_all_when_block_omitted() {
        let cfg: Config = toml::from_str(MINIMAL_TOML).unwrap();
        assert!(matches!(cfg.matching.mode, MatchMode::All));
    }

    #[test]
    fn matching_mode_best_is_parsed() {
        let toml = format!("{MINIMAL_TOML}\n[matching]\nmode = \"best\"\n");
        let cfg: Config = toml::from_str(&toml).unwrap();
        assert_eq!(cfg.matching.mode, MatchMode::Best);
    }

    #[test]
    fn matching_mode_all_is_parsed() {
        let toml = format!("{MINIMAL_TOML}\n[matching]\nmode = \"all\"\n");
        let cfg: Config = toml::from_str(&toml).unwrap();
        assert_eq!(cfg.matching.mode, MatchMode::All);
    }

    #[test]
    fn matching_defaults_to_all_when_mode_omitted() {
        let toml = format!("{MINIMAL_TOML}\n[matching]\n");
        let cfg: Config = toml::from_str(&toml).unwrap();
        assert!(matches!(cfg.matching.mode, MatchMode::All));
    }

    #[test]
    fn matching_mode_unknown_value_fails() {
        let toml = format!("{MINIMAL_TOML}\n[matching]\nmode = \"bogus\"\n");
        let result: std::result::Result<Config, _> = toml::from_str(&toml);
        assert!(result.is_err());
    }
}
