//! Certificate pinning (`pcs` link parameter).
//!
//! Some servers use self-signed certificates and publish the SHA-256 of the
//! certificate in the link. Xray pins certificate hashes natively; sing-box can only
//! pin public keys. For sing-box we connect to the server once, accept the handshake
//! only if the certificate matches the published hash (and the server proves it owns
//! the key), then derive the public-key hash from that very certificate.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use base64::Engine;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::model::{Profile, Transport};

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Accepts a server only if one of its certificates matches a pin.
#[derive(Debug)]
struct PinVerifier {
    pins: Vec<String>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let matches = std::iter::once(end_entity)
            .chain(intermediates)
            .any(|c| self.pins.contains(&sha256_hex(c)));
        if matches {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "server certificate does not match the pinned hash".into(),
            ))
        }
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
            &self.provider.signature_verification_algorithms,
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
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Fills `tls.pinned_pubkey_sha256` for a profile with certificate pins. No-op otherwise.
/// Blocking: performs one TLS handshake with the server.
pub fn resolve_pubkey_pins(profile: &mut Profile, timeout: Duration) -> anyhow::Result<()> {
    let transport_is_grpc = matches!(
        profile.transport,
        Transport::Grpc { .. } | Transport::Http { .. }
    );
    let Some(tls) = profile.tls.as_mut() else {
        return Ok(());
    };
    if tls.pinned_cert_sha256.is_empty() || !tls.pinned_pubkey_sha256.is_empty() {
        return Ok(());
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinVerifier {
        pins: tls.pinned_cert_sha256.clone(),
        provider: provider.clone(),
    });
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = if !tls.alpn.is_empty() {
        tls.alpn.iter().map(|a| a.as_bytes().to_vec()).collect()
    } else if transport_is_grpc {
        vec![b"h2".to_vec()]
    } else {
        Vec::new()
    };

    let sni = tls.sni.clone().unwrap_or_else(|| profile.server.clone());
    let name = ServerName::try_from(sni.clone()).with_context(|| format!("invalid SNI '{sni}'"))?;
    let mut conn = ClientConnection::new(Arc::new(config), name)?;

    let addr = (profile.server.as_str(), profile.port)
        .to_socket_addrs()
        .with_context(|| format!("cannot resolve {}", profile.server))?
        .next()
        .context("no address")?;
    let mut sock = TcpStream::connect_timeout(&addr, timeout).context("connect failed")?;
    sock.set_read_timeout(Some(timeout))?;
    sock.set_write_timeout(Some(timeout))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock).context(
            "TLS handshake failed (the server may only answer browser-like uTLS handshakes; \
             the Xray core supports such pinned profiles natively)",
        )?;
    }

    let certs = conn
        .peer_certificates()
        .context("server sent no certificate")?;
    let pinned = certs
        .iter()
        .find(|c| tls.pinned_cert_sha256.contains(&sha256_hex(c)))
        .context("no pinned certificate in chain")?;
    let (_, x509) = x509_parser::parse_x509_certificate(pinned)
        .map_err(|e| anyhow::anyhow!("bad certificate: {e}"))?;
    let spki = x509.tbs_certificate.subject_pki.raw;
    tls.pinned_pubkey_sha256 =
        vec![base64::engine::general_purpose::STANDARD.encode(Sha256::digest(spki))];

    conn.send_close_notify();
    let _ = conn.complete_io(&mut sock);
    Ok(())
}

/// Resolves pins for many profiles concurrently. Returns per-profile errors.
pub async fn resolve_many(
    profiles: &mut [Profile],
    timeout: Duration,
    concurrency: usize,
) -> Vec<(usize, String)> {
    use futures::StreamExt;
    let jobs: Vec<(usize, Profile)> = profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            p.tls.as_ref().is_some_and(|t| {
                !t.pinned_cert_sha256.is_empty() && t.pinned_pubkey_sha256.is_empty()
            })
        })
        .map(|(i, p)| (i, p.clone()))
        .collect();
    let results: Vec<(usize, Result<Profile, String>)> = futures::stream::iter(jobs)
        .map(|(i, mut p)| async move {
            let r = tokio::task::spawn_blocking(move || {
                resolve_pubkey_pins(&mut p, timeout).map(|_| p)
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| format!("{e:#}")));
            (i, r)
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;
    let mut errors = Vec::new();
    for (i, r) in results {
        match r {
            Ok(p) => profiles[i] = p,
            Err(e) => errors.push((i, e)),
        }
    }
    errors
}
