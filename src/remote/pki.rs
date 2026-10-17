//! The certificate authority behind remote control: it signs one server certificate per
//! start and one client certificate per paired phone. Nothing else is trusted.

use anyhow::Context;
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, CertificateSigningRequestDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

pub struct Ca {
    /// The certificate as stored and as phones pin it.
    pub der: CertificateDer<'static>,
    issuer: rcgen::Certificate,
    key: KeyPair,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Create `dir` readable by the owner only.
pub fn private_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Write a file readable by the owner only, replacing it in one step.
pub fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    // A name of its own: `ca.der` and `ca.key` would otherwise both be written through `ca.tmp`.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_file_name(format!(".{}.{}.{}.tmp", path.file_name().and_then(|f| f.to_str()).unwrap_or("file"), std::process::id(), n));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl Ca {
    pub fn load_or_create(dir: &Path) -> anyhow::Result<Ca> {
        private_dir(dir)?;
        // A forkdeck starting and `forkdeck remote pair` at once must not make two authorities.
        let _lock = crate::proc::FileLock::wait(&dir.join("ca.lock"), std::time::Duration::from_secs(10)).map_err(anyhow::Error::msg)?;
        let (cert_path, key_path) = (dir.join("ca.der"), dir.join("ca.key"));
        if cert_path.exists() && key_path.exists() {
            let der = CertificateDer::from(std::fs::read(&cert_path)?);
            let key = KeyPair::from_pem(&std::fs::read_to_string(&key_path)?)
                .with_context(|| format!("reading {}", key_path.display()))?;
            let params = CertificateParams::from_ca_cert_der(&der)
                .with_context(|| format!("reading {}", cert_path.display()))?;
            // Rebuilt only to sign with: same subject and key, so what it signs chains to `der`.
            let issuer = params.self_signed(&key)?;
            return Ok(Ca { der, issuer, key });
        }
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.distinguished_name.push(DnType::CommonName, format!("forkdeck remote CA ({})", crate::team::hostname()));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let issuer = params.self_signed(&key)?;
        let der = issuer.der().clone();
        write_private(&key_path, key.serialize_pem().as_bytes())?;
        write_private(&cert_path, &der)?;
        Ok(Ca { der, issuer, key })
    }

    pub fn fingerprint(&self) -> String {
        sha256_hex(&self.der)
    }

    /// A fresh server certificate for these addresses.
    pub fn server_cert(&self, ips: &[IpAddr]) -> anyhow::Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, "forkdeck");
        params.subject_alt_names = ips.iter().map(|ip| SanType::IpAddress(*ip)).collect();
        params.subject_alt_names.push(SanType::DnsName("localhost".try_into()?));
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let cert = params.signed_by(&key, &self.issuer, &self.key)?;
        Ok((cert.der().clone(), PrivatePkcs8KeyDer::from(key.serialize_der()).into()))
    }

    /// Sign a phone's certificate request. Only the public key is taken from it.
    pub fn sign_client(&self, csr: &[u8], device: &str) -> anyhow::Result<CertificateDer<'static>> {
        let mut req = CertificateSigningRequestParams::from_der(&CertificateSigningRequestDer::from(csr.to_vec()))
            .context("the certificate request does not parse")?;
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, device);
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        req.params = params;
        Ok(req.signed_by(&self.issuer, &self.key)?.der().clone())
    }

    /// TLS for the listener: any client certificate must chain to this CA, and a connection
    /// without one is let in for pairing only.
    pub fn server_config(&self, ips: &[IpAddr]) -> anyhow::Result<Arc<rustls::ServerConfig>> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(self.der.clone())?;
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider())
            .allow_unauthenticated()
            .build()?;
        let (cert, key) = self.server_cert(ips)?;
        let cfg = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()?
            .with_client_cert_verifier(verifier)
            // The CA goes along so a phone that only knows its fingerprint (from the QR code) can check it.
            .with_single_cert(vec![cert, self.der.clone()], key)?;
        Ok(Arc::new(cfg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reloaded_ca_signs_certificates_that_chain_to_the_stored_one() {
        let dir = std::env::temp_dir().join(format!("forkdeck-pki-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = Ca::load_or_create(&dir).unwrap();
        let again = Ca::load_or_create(&dir).unwrap();
        assert_eq!(first.der, again.der);
        assert_eq!(first.fingerprint().len(), 64);

        let phone = KeyPair::generate().unwrap();
        let csr = CertificateParams::default().serialize_request(&phone).unwrap();
        let cert = again.sign_client(csr.der(), "pixel").unwrap();

        let mut roots = rustls::RootCertStore::empty();
        roots.add(first.der.clone()).unwrap();
        let verifier =
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider()).build().unwrap();
        verifier
            .verify_client_cert(&cert, &[], rustls::pki_types::UnixTime::now())
            .expect("client certificate chains to the stored CA");

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("ca.key")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn junk_is_not_signed() {
        let dir = std::env::temp_dir().join(format!("forkdeck-pki-junk-{}", std::process::id()));
        let ca = Ca::load_or_create(&dir).unwrap();
        assert!(ca.sign_client(b"not a csr", "x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
