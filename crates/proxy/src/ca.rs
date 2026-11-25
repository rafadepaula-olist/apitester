use std::net::IpAddr;
use std::path::Path;

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType,
};

use crate::error::ProxyError;

const DEFAULT_CA_COMMON_NAME: &str = "apitester Local Proxy CA";

/// A dynamically generated leaf certificate signed by [`CertificateAuthority`].
#[derive(Debug, Clone)]
pub struct LeafCertificate {
    pub hostname: String,
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

/// Self-signed root CA for exporting/trusting and minting per-host leaf certificates.
pub struct CertificateAuthority {
    ca_cert: Certificate,
    ca_key: KeyPair,
    cert_pem: String,
    key_pem: String,
}

impl std::fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateAuthority")
            .field("cert_pem_len", &self.cert_pem.len())
            .finish()
    }
}

impl CertificateAuthority {
    /// Generates a new self-signed root CA with the default common name.
    pub fn generate_default() -> Result<Self, ProxyError> {
        Self::generate(DEFAULT_CA_COMMON_NAME)
    }

    /// Generates a new self-signed root CA with the given common name.
    pub fn generate(common_name: &str) -> Result<Self, ProxyError> {
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];

        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, common_name);
        dn.push(DnType::OrganizationName, "apitester");
        params.distinguished_name = dn;

        let ca_key = KeyPair::generate()?;
        let ca_cert = params.self_signed(&ca_key)?;
        let cert_pem = ca_cert.pem();
        let key_pem = ca_key.serialize_pem();

        Ok(Self {
            ca_cert,
            ca_key,
            cert_pem,
            key_pem,
        })
    }

    /// Loads an existing CA certificate and private key from PEM strings.
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self, ProxyError> {
        let ca_key = KeyPair::from_pem(key_pem)?;
        let params = CertificateParams::from_ca_cert_pem(cert_pem)?;
        let ca_cert = params.self_signed(&ca_key)?;

        Ok(Self {
            ca_cert,
            ca_key,
            cert_pem: cert_pem.to_string(),
            key_pem: key_pem.to_string(),
        })
    }

    /// Persists the root CA certificate and private key as PEM files.
    pub fn save(&self, cert_path: &Path, key_path: &Path) -> Result<(), ProxyError> {
        if let Some(parent) = cert_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        if let Some(parent) = key_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
                }
            }
        }
        std::fs::write(cert_path, &self.cert_pem)?;
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut file = opts.open(key_path)?;
            file.write_all(self.key_pem.as_bytes())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
            }
        }
        Ok(())
    }

    /// Loads an existing CA from disk, or generates and persists a new one if missing.
    pub fn load_or_generate(cert_path: &Path, key_path: &Path) -> Result<Self, ProxyError> {
        if cert_path.exists() && key_path.exists() {
            let cert_pem = std::fs::read_to_string(cert_path)?;
            let key_pem = std::fs::read_to_string(key_path)?;
            Self::from_pem(&cert_pem, &key_pem)
        } else {
            let ca = Self::generate_default()?;
            ca.save(cert_path, key_path)?;
            Ok(ca)
        }
    }

    /// Returns the root CA certificate in PEM format (for exporting/trusting).
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Returns the root CA private key in PEM format.
    pub fn key_pem(&self) -> &str {
        &self.key_pem
    }

    /// Generates an on-the-fly leaf certificate signed by this CA for `hostname`.
    pub fn sign_leaf(&self, hostname: &str) -> Result<LeafCertificate, ProxyError> {
        let host = hostname.trim();
        if host.is_empty() {
            return Err(ProxyError::InvalidRequest(
                "hostname must not be empty".to_string(),
            ));
        }

        let mut params = CertificateParams::default();
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, host);
        params.distinguished_name = dn;

        let san = if let Ok(ip) = host.parse::<IpAddr>() {
            SanType::IpAddress(ip)
        } else {
            SanType::DnsName(host.try_into()?)
        };
        params.subject_alt_names = vec![san];

        let leaf_key = KeyPair::generate()?;
        let leaf_cert = params.signed_by(&leaf_key, &self.ca_cert, &self.ca_key)?;

        Ok(LeafCertificate {
            hostname: host.to_string(),
            cert_pem: leaf_cert.pem(),
            key_pem: leaf_key.serialize_pem(),
            cert_der: leaf_cert.der().to_vec(),
            key_der: leaf_key.serialize_der(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_persist_reload_and_sign_leaf() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cert_path = dir.path().join("ca.crt.pem");
        let key_path = dir.path().join("ca.key.pem");

        let ca1 = CertificateAuthority::load_or_generate(&cert_path, &key_path).expect("generate");
        assert!(cert_path.exists());
        assert!(key_path.exists());
        assert!(ca1.cert_pem().contains("BEGIN CERTIFICATE"));
        assert!(ca1.key_pem().contains("BEGIN PRIVATE KEY"));

        // Second call loads the exact persisted PEMs.
        let ca2 = CertificateAuthority::load_or_generate(&cert_path, &key_path).expect("reload");
        assert_eq!(ca1.cert_pem(), ca2.cert_pem());
        assert_eq!(ca1.key_pem(), ca2.key_pem());

        // Sign leaf certs for both DNS hostnames and IP addresses.
        let dns_leaf = ca2.sign_leaf("api.example.com").expect("dns leaf");
        assert_eq!(dns_leaf.hostname, "api.example.com");
        assert!(dns_leaf.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(dns_leaf.key_pem.contains("BEGIN PRIVATE KEY"));
        assert!(!dns_leaf.cert_der.is_empty());
        assert!(!dns_leaf.key_der.is_empty());

        let ip_leaf = ca2.sign_leaf("127.0.0.1").expect("ip leaf");
        assert_eq!(ip_leaf.hostname, "127.0.0.1");
        assert!(!ip_leaf.cert_der.is_empty());
    }

    #[test]
    fn sign_leaf_rejects_empty_hostname() {
        let ca = CertificateAuthority::generate_default().expect("ca");
        assert!(ca.sign_leaf("   ").is_err());
    }
}
