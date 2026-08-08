#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod agent;
mod hub;

use std::path::Path;

pub use agent::{AgentConfig, load_agent_config};
use anyhow::Context;
pub use hub::{BerthConfig, TlsConfig, load_berth_config};
use serde::de::DeserializeOwned;

fn load<T: DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file at {}", path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file at {}", path.display()))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct Example {
        name: String,
    }

    fn tmp_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "berth-config-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos(),
        ))
    }

    #[test]
    fn parses_an_existing_file() {
        let path = tmp_path("existing");
        std::fs::write(&path, "name = \"pi\"").expect("write");
        let config: Example = load(&path).expect("load");
        assert_eq!(config.name, "pi");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_a_missing_file() {
        let path = tmp_path("missing");
        let err = load::<Example>(&path).expect_err("missing file must error");
        assert!(err.to_string().contains("failed to read config file"));
    }

    #[test]
    fn rejects_a_missing_required_field_in_an_existing_file() {
        let path = tmp_path("missing-field");
        std::fs::write(&path, "").expect("write");
        let err = load::<Example>(&path).expect_err("missing required field must error");
        assert!(err.to_string().contains("failed to parse config file"));
        std::fs::remove_file(&path).ok();
    }
}
