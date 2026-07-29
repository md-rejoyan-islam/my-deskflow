//! TLS / certificate plumbing.
//!
//! v1 generates a fresh self-signed certificate on first run, persists it
//! to disk, and surfaces the fingerprint to the user for TOFU pinning on
//! the peer.

use inputsync_core::{Error, Result};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct CertBundle {
    pub cert_chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub fingerprint_hex: String,
}

impl Clone for CertBundle {
    fn clone(&self) -> Self {
        Self {
            cert_chain: self.cert_chain.clone(),
            key: self.key.clone_key(),
            fingerprint_hex: self.fingerprint_hex.clone(),
        }
    }
}

/// Load a previously generated cert/key pair from `dir`, or generate a fresh
/// one if missing.
pub fn load_or_generate(dir: &Path) -> Result<CertBundle> {
    std::fs::create_dir_all(dir).map_err(Error::Io)?;
    let cert_path = dir.join("cert.der");
    let key_path = dir.join("key.der");

    if cert_path.exists() && key_path.exists() {
        let cert_bytes = std::fs::read(&cert_path)?;
        let key_bytes = std::fs::read(&key_path)?;
        let cert = CertificateDer::from(cert_bytes);
        let key: PrivateKeyDer<'static> =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_bytes));
        let fingerprint = sha256_hex(cert.as_ref());
        return Ok(CertBundle {
            cert_chain: vec![cert],
            key,
            fingerprint_hex: fingerprint,
        });
    }

    let cert = generate_simple_self_signed(vec!["inputsync.local".into()])
        .map_err(|e| Error::Network(format!("cert generation: {e}")))?;
    let cert_der = cert.cert.der().clone();
    let key_der = cert.key_pair.serialize_der();

    std::fs::write(&cert_path, cert_der.as_ref())?;
    std::fs::write(&key_path, &key_der)?;

    let fingerprint = sha256_hex(cert_der.as_ref());
    Ok(CertBundle {
        cert_chain: vec![cert_der],
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
        fingerprint_hex: fingerprint,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let h = blake3::hash(bytes);
    hex::encode(h.as_bytes())
}

pub fn server_rustls_config(bundle: &CertBundle) -> Result<rustls::ServerConfig> {
    let mut cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(bundle.cert_chain.clone(), bundle.key.clone_key())
        .map_err(|e| Error::Network(format!("rustls server config: {e}")))?;
    cfg.alpn_protocols = vec![b"inputsync/1".to_vec()];
    Ok(cfg)
}

pub fn client_rustls_config(pinned: Option<Vec<String>>) -> Result<rustls::ClientConfig> {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{ServerName, UnixTime};
    use rustls::DigitallySignedStruct;

    #[derive(Debug)]
    struct PinningVerifier {
        pinned: Vec<String>,
    }

    impl ServerCertVerifier for PinningVerifier {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> std::result::Result<ServerCertVerified, rustls::Error> {
            let fp = sha256_hex(end_entity.as_ref());
            if self.pinned.is_empty() || self.pinned.iter().any(|p| p.eq_ignore_ascii_case(&fp)) {
                Ok(ServerCertVerified::assertion())
            } else {
                Err(rustls::Error::General(format!(
                    "server cert fingerprint {fp} not in pin list"
                )))
            }
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            vec![
                rustls::SignatureScheme::RSA_PKCS1_SHA256,
                rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
                rustls::SignatureScheme::ED25519,
            ]
        }
    }

    let mut cfg = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinningVerifier {
            pinned: pinned.unwrap_or_default(),
        }))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"inputsync/1".to_vec()];
    Ok(cfg)
}

#[derive(Debug, Clone)]
pub struct CertPaths {
    pub dir: PathBuf,
}
