use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_hub_addr")]
    pub hub_addr: String,
    /// Pins the hub's gRPC certificate by SHA-256 fingerprint. Unset, the
    /// agent trusts whatever the hub presents on its first connect and
    /// persists that fingerprint for future connects instead.
    #[serde(default)]
    pub hub_tls_fingerprint: Option<String>,
}

impl Config {
    /// The file is entirely optional today - every field defaults - so a
    /// missing file is not an error, unlike `oxde-hub`'s config.
    pub fn load() -> anyhow::Result<Self> {
        let path =
            std::env::var("OXDE_AGENT_CONFIG").unwrap_or_else(|_| "oxde-agent.toml".to_string());
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => {
                return Err(err).with_context(|| format!("failed to read config file at {path}"));
            }
        };
        toml::from_str(&contents).with_context(|| format!("failed to parse config file at {path}"))
    }
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("agent-data")
}

fn default_hub_addr() -> String {
    format!("127.0.0.1:{}", oxde_proto::AGENT_GRPC_PORT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config: Config = toml::from_str("").expect("empty config should parse");
        assert_eq!(config.data_dir, default_data_dir());
        assert_eq!(config.hub_addr, default_hub_addr());
        assert_eq!(config.hub_tls_fingerprint, None);
    }

    #[test]
    fn fields_override_their_defaults() {
        let config: Config = toml::from_str(
            r#"
            data_dir = "/var/lib/oxde-agent"
            hub_addr = "hub.internal:50051"
            hub_tls_fingerprint = "abc123"
            "#,
        )
        .expect("config should parse");
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/oxde-agent"));
        assert_eq!(config.hub_addr, "hub.internal:50051");
        assert_eq!(config.hub_tls_fingerprint, Some("abc123".to_string()));
    }
}
