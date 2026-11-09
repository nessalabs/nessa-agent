//! Test-only certificates and keys, generated in memory for one recipient.
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use time::{Duration, OffsetDateTime};

pub struct RecipientTls {
    pub root: CertificateDer<'static>,
    pub config: ServerConfig,
}

impl RecipientTls {
    pub fn generate() -> Self {
        let now = OffsetDateTime::now_utc();
        let mut authority = CertificateParams::default();
        authority.not_before = now - Duration::days(1);
        authority.not_after = now + Duration::days(7);
        authority
            .distinguished_name
            .push(DnType::CommonName, "Nessa isolated webhook test CA");
        authority.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        authority.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let authority_key = KeyPair::generate().unwrap();
        let root = authority.self_signed(&authority_key).unwrap();
        let issuer = Issuer::from_params(&authority, &authority_key);

        let mut recipient = CertificateParams::new(vec!["recipient.test".into()]).unwrap();
        recipient.not_before = authority.not_before;
        recipient.not_after = authority.not_after;
        recipient
            .distinguished_name
            .push(DnType::CommonName, "recipient.test");
        recipient.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        recipient.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().unwrap();
        let certificate = recipient.signed_by(&key, &issuer).unwrap();
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .unwrap();
        Self {
            root: root.der().clone(),
            config,
        }
    }
}
