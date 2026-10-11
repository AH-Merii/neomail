//! TLS for IMAP: rustls with the platform's trust store, or an exact certificate pin.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use rustls_platform_verifier::BuilderVerifierExt;
use sha2::{Digest, Sha256};

/// The SHA-256 of a server's DER certificate. When set, it replaces CA verification: the
/// connection is accepted only if the certificate matches exactly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CertPin([u8; 32]);

impl CertPin {
    pub fn of(cert_der: &[u8]) -> Self {
        Self(Sha256::digest(cert_der).into())
    }
}

impl fmt::Display for CertPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for CertPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CertPin({self})")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "cert_sha256 must be 64 hex digits (colons allowed), as printed by `openssl x509 -fingerprint -sha256`"
)]
pub struct InvalidPin;

impl FromStr for CertPin {
    type Err = InvalidPin;

    /// Accepts `ab12…` or `AB:12:…`.
    fn from_str(s: &str) -> Result<Self, InvalidPin> {
        let hex: Vec<u8> = s.trim().bytes().filter(|b| *b != b':').collect();
        // Only ASCII hex digits: `u8::from_str_radix` alone would also take a sign, as in "+f".
        if hex.len() != 64 || !hex.iter().all(u8::is_ascii_hexdigit) {
            return Err(InvalidPin);
        }
        let mut out = [0u8; 32];
        for (byte, pair) in out.iter_mut().zip(hex.as_chunks::<2>().0) {
            let digits = std::str::from_utf8(pair.as_slice()).map_err(|_| InvalidPin)?;
            *byte = u8::from_str_radix(digits, 16).map_err(|_| InvalidPin)?;
        }
        Ok(Self(out))
    }
}

/// Builds the client config: a pin if given, else the platform verifier. There is deliberately
/// no way to turn verification off.
pub fn client_config(pin: Option<CertPin>) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?;
    let config = match pin {
        Some(pin) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinVerifier::new(pin, &provider)))
            .with_no_client_auth(),
        None => builder.with_platform_verifier()?.with_no_client_auth(),
    };
    Ok(Arc::new(config))
}

/// Accepts exactly one certificate, by its SHA-256, and still checks the handshake signatures
/// against that certificate's key, so the peer must hold its private key.
#[derive(Debug)]
struct PinVerifier {
    pin: CertPin,
    algorithms: WebPkiSupportedAlgorithms,
}

impl PinVerifier {
    fn new(pin: CertPin, provider: &CryptoProvider) -> Self {
        Self {
            pin,
            algorithms: provider.signature_verification_algorithms,
        }
    }
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual = CertPin::of(end_entity.as_ref());
        if actual == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "the server certificate's SHA-256 is {actual}, which does not match cert_sha256 {}",
                self.pin
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_parses_plain_and_colon_hex() {
        let plain = "00112233445566778899aabbccddeeff0123456789abcdef00112233445566ff";
        let pin: CertPin = plain.parse().unwrap();
        assert_eq!(pin.to_string(), plain);
        let colons = plain
            .as_bytes()
            .chunks(2)
            .map(|c| std::str::from_utf8(c).unwrap().to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join(":");
        assert_eq!(colons.parse::<CertPin>().unwrap(), pin);
    }

    #[test]
    fn bad_pins_are_rejected() {
        for bad in [
            "",
            "abc",
            &"g".repeat(64),
            &"a".repeat(66),
            "é".repeat(32).as_str(),
            // A sign inside a pair: from_str_radix would read "+f" as 15.
            &format!("+f{}", "a".repeat(62)),
            &format!("{}+f", "a".repeat(62)),
        ] {
            assert_eq!(bad.parse::<CertPin>(), Err(InvalidPin), "{bad}");
        }
    }

    #[test]
    fn pinned_verifier_accepts_only_the_pinned_cert() {
        let provider = rustls::crypto::ring::default_provider();
        let cert = CertificateDer::from(b"not really DER".to_vec());
        let verifier = PinVerifier::new(CertPin::of(cert.as_ref()), &provider);
        let name = ServerName::try_from("127.0.0.1").unwrap();
        let now = UnixTime::now();
        assert!(
            verifier
                .verify_server_cert(&cert, &[], &name, &[], now)
                .is_ok()
        );
        let other = CertificateDer::from(b"another cert".to_vec());
        assert!(
            verifier
                .verify_server_cert(&other, &[], &name, &[], now)
                .is_err()
        );
    }

    #[test]
    fn both_configs_build() {
        assert!(client_config(Some(CertPin::of(b"x"))).is_ok());
        assert!(client_config(None).is_ok());
    }
}
