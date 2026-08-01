use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AgentConfig {
    pub data_dir: PathBuf,
    /// Pairing token from the hub's `POST /api/hosts`, sent as gRPC
    /// metadata on `Session` to identify this agent as a `Host` row.
    pub agent_token: String,
    #[serde(default = "default_hub_addr")]
    pub hub_addr: String,
    /// Pins the hub's gRPC certificate by SHA-256 fingerprint. Unset, the
    /// agent trusts whatever the hub presents on its first connect and
    /// persists that fingerprint for future connects instead.
    #[serde(default)]
    pub hub_tls_fingerprint: Option<String>,
}

/// # Errors
///
/// Returns an error if `oxde-agent.toml` (or `$OXDE_AGENT_CONFIG`) can't be
/// read or parsed.
pub fn load_agent_config() -> anyhow::Result<AgentConfig> {
    let path = std::env::var("OXDE_AGENT_CONFIG").unwrap_or_else(|_| "oxde-agent.toml".to_string());
    crate::load(Path::new(&path))
}

fn default_hub_addr() -> String {
    format!("127.0.0.1:{}", oxde_proto::AGENT_GRPC_PORT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_is_required() {
        let err = toml::from_str::<AgentConfig>(r#"agent_token = "tok""#)
            .expect_err("data_dir must be required");
        assert!(err.to_string().contains("data_dir"));
    }

    #[test]
    fn agent_token_is_required() {
        let err = toml::from_str::<AgentConfig>(r#"data_dir = "/var/lib/oxde-agent""#)
            .expect_err("agent_token must be required");
        assert!(err.to_string().contains("agent_token"));
    }

    #[test]
    fn optional_fields_fall_back_to_defaults() {
        let config: AgentConfig = toml::from_str(
            r#"
            data_dir = "/var/lib/oxde-agent"
            agent_token = "tok"
            "#,
        )
        .expect("config should parse");
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/oxde-agent"));
        assert_eq!(config.agent_token, "tok");
        assert_eq!(config.hub_addr, default_hub_addr());
        assert_eq!(config.hub_tls_fingerprint, None);
    }

    #[test]
    fn fields_override_their_defaults() {
        let config: AgentConfig = toml::from_str(
            r#"
            data_dir = "/var/lib/oxde-agent"
            agent_token = "tok"
            hub_addr = "hub.internal:50051"
            hub_tls_fingerprint = "abc123"
            "#,
        )
        .expect("config should parse");
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/oxde-agent"));
        assert_eq!(config.agent_token, "tok");
        assert_eq!(config.hub_addr, "hub.internal:50051");
        assert_eq!(config.hub_tls_fingerprint, Some("abc123".to_string()));
    }
}
