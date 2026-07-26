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
    #[serde(default = "default_build_timeout_secs")]
    pub build_timeout_secs: u64,
    #[serde(default = "default_api_token_max_expiry_days")]
    pub api_token_max_expiry_days: i64,
    #[serde(default)]
    pub enable_mcp: bool,
    #[serde(default)]
    pub tls: TlsConfig,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    #[serde(default = "default_https_port")]
    pub https_port: u16,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = std::env::var("OXDE_CONFIG").unwrap_or_else(|_| "oxde.toml".to_string());
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file at {path}"))?;
        toml::from_str(&contents).with_context(|| format!("failed to parse config file at {path}"))
    }
}

/// `Off` uses `http_port`, `Manual` uses `https_port` - both configurable
/// so local dev can use unprivileged ports.
#[derive(Debug, Default, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum TlsConfig {
    #[default]
    Off,
    Manual {
        cert_path: PathBuf,
        key_path: PathBuf,
    },
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

const fn default_build_timeout_secs() -> u64 {
    300
}

const fn default_api_token_max_expiry_days() -> i64 {
    30
}

const fn default_http_port() -> u16 {
    80
}

const fn default_https_port() -> u16 {
    443
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_config_defaults_to_off_when_absent_from_config() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            tls: TlsConfig,
        }
        let wrapper: Wrapper = toml::from_str("").unwrap();
        assert!(matches!(wrapper.tls, TlsConfig::Off));
    }

    #[test]
    fn tls_config_manual_mode_round_trips() {
        let tls: TlsConfig = toml::from_str(
            r#"
            mode = "manual"
            cert_path = "/etc/oxde/tls/cert.pem"
            key_path = "/etc/oxde/tls/key.pem"
            "#,
        )
        .unwrap();
        match tls {
            TlsConfig::Manual {
                cert_path,
                key_path,
            } => {
                assert_eq!(cert_path, PathBuf::from("/etc/oxde/tls/cert.pem"));
                assert_eq!(key_path, PathBuf::from("/etc/oxde/tls/key.pem"));
            }
            TlsConfig::Off => panic!("expected TlsConfig::Manual"),
        }
    }
}
