use std::{
    fmt::Write,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use rustls::{
    DigitallySignedStruct, Error, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};

fn pin_path(data_dir: &Path) -> PathBuf {
    data_dir.join("hub-tls-fingerprint.txt")
}

/// Priority order: an explicit config override, then whatever was
/// persisted from a prior trust-on-first-connect, then `None` - a genuine
/// first connection, which accepts whatever's presented.
pub fn expected_fingerprint(data_dir: &Path, configured: Option<String>) -> Option<String> {
    if let Some(pinned) = configured {
        return Some(pinned.trim().to_lowercase());
    }
    std::fs::read_to_string(pin_path(data_dir))
        .ok()
        .map(|contents| contents.trim().to_string())
}

/// Only called after a trust-on-first-connect with no prior pin and no
/// explicit override, so this never overwrites an existing pin.
pub fn persist_fingerprint(data_dir: &Path, fingerprint: &str) -> std::io::Result<()> {
    std::fs::write(pin_path(data_dir), fingerprint)
}

#[derive(Debug)]
pub struct FingerprintVerifier {
    provider: CryptoProvider,
    expected: Option<String>,
    captured: Mutex<Option<String>>,
}

impl FingerprintVerifier {
    pub fn new(expected: Option<String>) -> Self {
        Self {
            provider: rustls::crypto::ring::default_provider(),
            expected,
            captured: Mutex::new(None),
        }
    }

    pub fn captured_fingerprint(&self) -> Option<String> {
        self.captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let actual = hex_sha256(end_entity);
        if let Some(expected) = &self.expected
            && *expected != actual
        {
            return Err(Error::General(format!(
                "hub certificate fingerprint mismatch (expected {expected}, got {actual})"
            )));
        }
        *self.captured.lock().unwrap_or_else(PoisonError::into_inner) = Some(actual);
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn hex_sha256(der: &CertificateDer<'_>) -> String {
    Sha256::digest(der.as_ref())
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use std::{pin::Pin, sync::Arc, time::SystemTime};

    use berth_proto::hub::v1::{
        PingRequest, PingResponse, SessionRequest, SessionResponse,
        hub_service_server::{HubService, HubServiceServer},
    };
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use tokio_stream::Stream;
    use tonic::{
        Request, Response, Status, Streaming,
        transport::{
            Channel, ClientTlsConfig, Endpoint, Identity, Server, ServerTlsConfig,
            server::TcpIncoming,
        },
    };

    use super::*;

    struct StubHub;

    #[tonic::async_trait]
    impl HubService for StubHub {
        async fn ping(
            &self,
            _request: Request<PingRequest>,
        ) -> Result<Response<PingResponse>, Status> {
            Ok(Response::new(PingResponse {
                version: "test".to_string(),
            }))
        }

        type SessionStream = Pin<Box<dyn Stream<Item = Result<SessionResponse, Status>> + Send>>;

        async fn session(
            &self,
            _request: Request<Streaming<SessionRequest>>,
        ) -> Result<Response<Self::SessionStream>, Status> {
            Err(Status::unimplemented("not exercised by these tests"))
        }
    }

    /// Starts a real TLS-enabled test server, returning its address and
    /// the exact fingerprint an agent should see when pinning it.
    fn spawn_test_server() -> (String, String) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let incoming = TcpIncoming::bind("127.0.0.1:0".parse().expect("addr")).expect("bind");
        let addr = incoming.local_addr().expect("local_addr");

        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["berth-agent-link-test".to_string()])
                .expect("generate cert");
        let fingerprint = hex_sha256(cert.der());
        let identity = Identity::from_pem(cert.pem(), signing_key.serialize_pem());

        tokio::spawn(async move {
            Server::builder()
                .tls_config(ServerTlsConfig::new().identity(identity))
                .expect("tls_config")
                .add_service(HubServiceServer::new(StubHub))
                .serve_with_incoming(incoming)
                .await
                .expect("serve");
        });

        (format!("https://{addr}"), fingerprint)
    }

    async fn connect(
        hub_addr: &str,
        verifier: Arc<FingerprintVerifier>,
    ) -> Result<Channel, tonic::transport::Error> {
        Endpoint::from_shared(hub_addr.to_string())
            .expect("uri")
            .tls_config_with_verifier(ClientTlsConfig::new(), verifier)
            .expect("tls_config")
            .connect()
            .await
    }

    #[tokio::test]
    async fn connects_when_the_pinned_fingerprint_matches() {
        let (hub_addr, fingerprint) = spawn_test_server();
        let verifier = Arc::new(FingerprintVerifier::new(Some(fingerprint)));
        connect(&hub_addr, verifier).await.expect("should connect");
    }

    #[tokio::test]
    async fn refuses_to_connect_when_the_pinned_fingerprint_does_not_match() {
        let (hub_addr, _fingerprint) = spawn_test_server();
        let verifier = Arc::new(FingerprintVerifier::new(Some("0".repeat(64))));
        let result = connect(&hub_addr, verifier).await;
        assert!(
            result.is_err(),
            "a mismatched pin must reject the connection"
        );
    }

    #[tokio::test]
    async fn trust_on_first_connect_accepts_and_captures_the_fingerprint() {
        let (hub_addr, fingerprint) = spawn_test_server();
        let verifier = Arc::new(FingerprintVerifier::new(None));
        connect(&hub_addr, verifier.clone())
            .await
            .expect("should connect");
        assert_eq!(verifier.captured_fingerprint(), Some(fingerprint));
    }

    fn test_data_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "berth-agent-test-hub-tls-{label}-{}-{nanos}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("create test data dir");
        dir
    }

    #[test]
    fn expected_fingerprint_is_none_when_nothing_is_pinned() {
        let data_dir = test_data_dir("none");
        assert_eq!(expected_fingerprint(&data_dir, None), None);
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn expected_fingerprint_returns_the_persisted_pin_when_present() {
        let data_dir = test_data_dir("persisted");
        persist_fingerprint(&data_dir, "abc123").expect("persist");
        assert_eq!(
            expected_fingerprint(&data_dir, None),
            Some("abc123".to_string())
        );
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn expected_fingerprint_prefers_the_configured_override_over_a_persisted_pin() {
        let data_dir = test_data_dir("config-override");
        persist_fingerprint(&data_dir, "persisted").expect("persist");
        let result = expected_fingerprint(&data_dir, Some("FromConfig".to_string()));
        assert_eq!(result, Some("fromconfig".to_string()));
        std::fs::remove_dir_all(&data_dir).ok();
    }
}
