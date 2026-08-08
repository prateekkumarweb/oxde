use std::{
    fmt::Write,
    path::{Path, PathBuf},
};

use anyhow::Context;
use sha2::{Digest, Sha256};
use tonic::transport::Identity;

/// The hub<->agent gRPC channel's TLS identity - self-signed, since the
/// agent pins this certificate's fingerprint directly (trust-on-first-
/// connect) rather than validating it against a CA.
pub struct AgentTls {
    pub identity: Identity,
    pub fingerprint_hex: String,
}

/// Loads a persisted cert/key from `data_dir/agent-tls/`, generating and
/// persisting a fresh self-signed one on first startup so the fingerprint
/// stays stable across restarts.
pub fn load_or_generate(data_dir: &Path) -> anyhow::Result<AgentTls> {
    let dir = data_dir.join("agent-tls");
    let paths = Paths::new(&dir);

    let (cert_pem, key_pem, fingerprint_hex) = match paths.read_all() {
        Some(existing) => existing,
        None => generate_and_persist(&dir, &paths)?,
    };

    Ok(AgentTls {
        identity: Identity::from_pem(cert_pem, key_pem),
        fingerprint_hex,
    })
}

struct Paths {
    cert: PathBuf,
    key: PathBuf,
    fingerprint: PathBuf,
}

impl Paths {
    fn new(dir: &Path) -> Self {
        Self {
            cert: dir.join("cert.pem"),
            key: dir.join("key.pem"),
            fingerprint: dir.join("fingerprint.txt"),
        }
    }

    fn read_all(&self) -> Option<(String, String, String)> {
        let cert = std::fs::read_to_string(&self.cert).ok()?;
        let key = std::fs::read_to_string(&self.key).ok()?;
        let fingerprint = std::fs::read_to_string(&self.fingerprint).ok()?;
        Some((cert, key, fingerprint.trim().to_string()))
    }
}

fn generate_and_persist(dir: &Path, paths: &Paths) -> anyhow::Result<(String, String, String)> {
    std::fs::create_dir_all(dir).context("failed to create agent-tls dir")?;
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["berth-agent-link".to_string()])
            .context("failed to generate self-signed hub<->agent certificate")?;
    let fingerprint_hex = hex_sha256(cert.der());
    let cert_pem = cert.pem();
    let key_pem = signing_key.serialize_pem();

    std::fs::write(&paths.cert, &cert_pem).context("failed to write hub<->agent cert")?;
    std::fs::write(&paths.key, &key_pem).context("failed to write hub<->agent key")?;
    std::fs::write(&paths.fingerprint, &fingerprint_hex)
        .context("failed to write hub<->agent cert fingerprint")?;

    Ok((cert_pem, key_pem, fingerprint_hex))
}

fn hex_sha256(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_data_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "berth-hub-test-agent-tls-{label}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("create test data dir");
        dir
    }

    #[test]
    fn load_or_generate_persists_a_stable_fingerprint_across_calls() {
        let data_dir = test_data_dir("stable");
        let first = load_or_generate(&data_dir).expect("first load generates");
        let second = load_or_generate(&data_dir).expect("second load reuses");
        assert_eq!(first.fingerprint_hex, second.fingerprint_hex);
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn load_or_generate_uses_a_different_fingerprint_per_data_dir() {
        let a = test_data_dir("a");
        let b = test_data_dir("b");
        let cert_a = load_or_generate(&a).expect("load a");
        let cert_b = load_or_generate(&b).expect("load b");
        assert_ne!(cert_a.fingerprint_hex, cert_b.fingerprint_hex);
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }
}
