use std::{path::PathBuf, time::Duration};

use anyhow::Context;
use axum_server::tls_rustls::RustlsConfig;

/// Poll interval for [`spawn_reload_watcher`] - plain polling avoids
/// pulling in the `notify`/inotify crate for something this infrequent.
const RELOAD_POLL_INTERVAL: Duration = Duration::from_secs(30);

pub async fn build_rustls_config(
    cert_path: &PathBuf,
    key_path: &PathBuf,
) -> anyhow::Result<RustlsConfig> {
    RustlsConfig::from_pem_file(cert_path, key_path)
        .await
        .with_context(|| {
            format!(
                "failed to load TLS cert/key from {} / {}",
                cert_path.display(),
                key_path.display()
            )
        })
}

/// Reloads `config` whenever `cert_path`/`key_path`'s mtime changes, so an
/// externally renewed cert takes effect without restarting `OxDe`. A
/// failed reload is logged and skipped, keeping the previous cert active.
pub fn spawn_reload_watcher(config: RustlsConfig, cert_path: PathBuf, key_path: PathBuf) {
    tokio::spawn(async move {
        let mut last_seen = combined_mtime(&cert_path, &key_path);
        loop {
            tokio::time::sleep(RELOAD_POLL_INTERVAL).await;
            last_seen = check_and_reload(&config, &cert_path, &key_path, last_seen).await;
        }
    });
}

/// One poll tick, split out from [`spawn_reload_watcher`] so it's testable
/// without waiting on real sleeps. Returns the mtime to compare next.
async fn check_and_reload(
    config: &RustlsConfig,
    cert_path: &PathBuf,
    key_path: &PathBuf,
    last_seen: Option<(std::time::SystemTime, std::time::SystemTime)>,
) -> Option<(std::time::SystemTime, std::time::SystemTime)> {
    let current = combined_mtime(cert_path, key_path);
    if current == last_seen {
        return last_seen;
    }

    match config.reload_from_pem_file(cert_path, key_path).await {
        Ok(()) => tracing::info!(
            cert = %cert_path.display(),
            key = %key_path.display(),
            "reloaded TLS certificate"
        ),
        Err(err) => tracing::error!(
            error = %err,
            cert = %cert_path.display(),
            key = %key_path.display(),
            "failed to reload TLS certificate, keeping the previous one"
        ),
    }
    current
}

fn combined_mtime(
    cert_path: &PathBuf,
    key_path: &PathBuf,
) -> Option<(std::time::SystemTime, std::time::SystemTime)> {
    let cert_mtime = std::fs::metadata(cert_path).ok()?.modified().ok()?;
    let key_mtime = std::fs::metadata(key_path).ok()?.modified().ok()?;
    Some((cert_mtime, key_mtime))
}

#[cfg(test)]
mod tests {
    use rcgen::{CertifiedKey, generate_simple_self_signed};

    use super::*;

    fn write_self_signed_cert(dir: &std::path::Path, label: &str) -> (PathBuf, PathBuf) {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["oxde-test".to_string()])
                .expect("generate self-signed test cert");
        let cert_path = dir.join(format!("{label}-cert.pem"));
        let key_path = dir.join(format!("{label}-key.pem"));
        std::fs::write(&cert_path, cert.pem()).expect("write test cert");
        std::fs::write(&key_path, signing_key.serialize_pem()).expect("write test key");
        (cert_path, key_path)
    }

    /// `RustlsConfig::from_pem_file` needs a crypto provider installed -
    /// ignores the result since another test may have installed one first.
    fn install_test_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[tokio::test]
    async fn build_rustls_config_loads_a_valid_cert_and_key() {
        install_test_crypto_provider();
        let dir = tempdir();
        let (cert_path, key_path) = write_self_signed_cert(dir.path(), "initial");
        build_rustls_config(&cert_path, &key_path)
            .await
            .expect("valid cert/key should load");
    }

    #[tokio::test]
    async fn build_rustls_config_reports_missing_files() {
        let dir = tempdir();
        let result = build_rustls_config(
            &dir.path().join("missing-cert.pem"),
            &dir.path().join("missing-key.pem"),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn check_and_reload_skips_when_files_are_unchanged() {
        install_test_crypto_provider();
        let dir = tempdir();
        let (cert_path, key_path) = write_self_signed_cert(dir.path(), "unchanged");
        let config = build_rustls_config(&cert_path, &key_path).await.unwrap();
        let inner_before = config.get_inner();

        let last_seen = combined_mtime(&cert_path, &key_path);
        let result = check_and_reload(&config, &cert_path, &key_path, last_seen).await;

        assert_eq!(result, last_seen);
        assert!(std::sync::Arc::ptr_eq(&inner_before, &config.get_inner()));
    }

    #[tokio::test]
    async fn check_and_reload_reloads_when_files_change() {
        install_test_crypto_provider();
        let dir = tempdir();
        let (cert_path, key_path) = write_self_signed_cert(dir.path(), "before");
        let config = build_rustls_config(&cert_path, &key_path).await.unwrap();
        let inner_before = config.get_inner();

        // Simulate an external renewal: same paths, new content/mtime.
        let (new_cert_path, new_key_path) = write_self_signed_cert(dir.path(), "after");
        std::fs::copy(&new_cert_path, &cert_path).unwrap();
        std::fs::copy(&new_key_path, &key_path).unwrap();
        // Force a distinct mtime - coarse filesystem resolution can
        // otherwise land the copy on the same timestamp.
        let future = std::time::SystemTime::now() + Duration::from_mins(1);
        std::fs::File::open(&cert_path)
            .unwrap()
            .set_modified(future)
            .unwrap();
        std::fs::File::open(&key_path)
            .unwrap()
            .set_modified(future)
            .unwrap();

        let last_seen = combined_mtime(&cert_path, &key_path);
        let result = check_and_reload(&config, &cert_path, &key_path, None).await;

        assert_eq!(result, last_seen);
        assert!(!std::sync::Arc::ptr_eq(&inner_before, &config.get_inner()));
    }

    /// A fresh scratch dir per test, cleaned up on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "oxde-test-tls-{label}-{}-{}",
                std::process::id(),
                jiff::Timestamp::now().as_nanosecond()
            ));
            std::fs::create_dir_all(&dir).expect("create test tempdir");
            Self(dir)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> TempDir {
        TempDir::new("case")
    }
}
