use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub data_dir: PathBuf,
    pub admin_username: String,
    pub admin_password: String,
    pub base_domain: String,
    #[serde(default = "default_max_upload_bytes")]
    pub max_upload_bytes: u64,
    #[serde(default = "default_max_uncompressed_bytes")]
    pub max_uncompressed_bytes: u64,
    #[serde(default = "default_git_fetch_timeout_secs")]
    pub git_fetch_timeout_secs: u64,
    #[serde(default = "default_install_timeout_secs")]
    pub install_timeout_secs: u64,
}

const fn default_max_upload_bytes() -> u64 {
    200 * 1024 * 1024
}

const fn default_max_uncompressed_bytes() -> u64 {
    1024 * 1024 * 1024
}

const fn default_git_fetch_timeout_secs() -> u64 {
    60
}

const fn default_install_timeout_secs() -> u64 {
    300
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = std::env::var("OXDE_CONFIG").unwrap_or_else(|_| "oxde.toml".to_string());
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file at {path}"))?;
        toml::from_str(&contents).with_context(|| format!("failed to parse config file at {path}"))
    }
}
