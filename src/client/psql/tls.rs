//! TLS for PostgreSQL connections, on rustls.
//!
//! `prefer` and `require` encrypt without checking the certificate, as libpq
//! does; `verify-full` checks the chain and the host name. Whether TLS is tried
//! at all, and the fall back for `prefer`, is the driver's `ssl_mode`.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio_postgres_rustls::MakeRustlsConnect;

use super::{PsqlOptions, SslMode};
use crate::client::ClientError;

/// Accepts any certificate: encryption without authentication.
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls_err(e: impl std::fmt::Display) -> ClientError {
    ClientError::Open(format!("TLS setup failed: {e}"))
}

/// Roots for `verify-full`: the file given as `sslrootcert`, else the system's.
fn roots(sslrootcert: Option<&str>) -> Result<RootCertStore, ClientError> {
    let mut store = RootCertStore::empty();
    match sslrootcert {
        Some(path) => {
            let certs = CertificateDer::pem_file_iter(path)
                .map_err(|e| tls_err(format!("cannot read root certificate {path}: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| tls_err(format!("invalid root certificate {path}: {e}")))?;
            let (added, _) = store.add_parsable_certificates(certs);
            if added == 0 {
                return Err(tls_err(format!("no certificate found in {path}")));
            }
        }
        None => {
            let native = rustls_native_certs::load_native_certs();
            let (added, _) = store.add_parsable_certificates(native.certs);
            if added == 0 {
                return Err(tls_err(
                    "no system root certificates found; pass --sslrootcert",
                ));
            }
        }
    }
    Ok(store)
}

/// The TLS connector for `o`, or `None` for `disable`.
pub fn connector(o: &PsqlOptions) -> Result<Option<MakeRustlsConnect>, ClientError> {
    if o.sslmode == SslMode::Disable {
        return Ok(None);
    }
    let provider = Arc::new(ring::default_provider());
    let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(tls_err)?;
    let config = if o.sslmode == SslMode::VerifyFull {
        builder
            .with_root_certificates(roots(o.sslrootcert.as_deref())?)
            .with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth()
    };
    Ok(Some(MakeRustlsConnect::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(mode: SslMode, root: Option<&str>) -> PsqlOptions {
        PsqlOptions {
            sslmode: mode,
            sslrootcert: root.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn disable_has_no_connector() {
        assert!(connector(&opts(SslMode::Disable, None)).unwrap().is_none());
    }

    #[test]
    fn prefer_and_require_build_without_roots() {
        for m in [SslMode::Prefer, SslMode::Require] {
            assert!(connector(&opts(m, Some("/nonexistent"))).unwrap().is_some());
        }
    }

    #[test]
    fn verify_full_needs_a_readable_root_certificate() {
        let err = connector(&opts(SslMode::VerifyFull, Some("/nonexistent/ca.pem")))
            .err()
            .unwrap();
        assert!(err.to_string().contains("/nonexistent/ca.pem"), "{err}");
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "not a certificate\n").unwrap();
        let err = connector(&opts(SslMode::VerifyFull, Some(empty.to_str().unwrap())))
            .err()
            .unwrap();
        assert!(err.to_string().contains("no certificate"), "{err}");
    }
}
