//! 待ち受けの TLS。自己署名の証明書は作って置き場に残し、指紋を登録文字列に載せる。

use super::durable::StagedWrite;
use axum_server::tls_rustls::RustlsConfig;
use chrono::{DateTime, Utc};
use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use x509_parser::parse_x509_certificate;

const SELF_SIGNED_RECOVERY: &str = "自己署名の証明書と秘密鍵を両方削除すると、次の起動で作り直します。登録済みの端末は再登録してください";

pub struct TlsMaterial {
    pub config: RustlsConfig,
    pub certificate: CertificateInfo,
    /// 自己署名のときだけ。OS が信頼する CA の証明書はホスト名で検証する。
    pub fingerprint: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateInfo {
    pub expires_at: DateTime<Utc>,
    pub source: CertificateSource,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CertificateSource {
    Files,
    SelfSigned,
}

fn install_crypto_provider() {
    // 他の依存が rustls の暗号実装を両方有効にするので、明示的に選ぶ。
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

pub fn certificate_fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn material_from_pem(
    certificate: Vec<u8>,
    private_key: Vec<u8>,
    source: CertificateSource,
) -> Result<TlsMaterial, String> {
    // 待ち受けに渡す証明書列の先頭が、サーバー自身の証明書。
    let leaf = CertificateDer::pem_slice_iter(&certificate)
        .next()
        .ok_or("証明書が空です")?
        .map_err(|error| format!("証明書を解析できません: {error}"))?;
    let (_, parsed) = parse_x509_certificate(&leaf)
        .map_err(|error| format!("証明書を解析できません: {error}"))?;
    let expires_at = DateTime::from_timestamp(parsed.validity().not_after.timestamp(), 0)
        .ok_or("証明書の有効期限が範囲外です")?;
    let fingerprint =
        matches!(source, CertificateSource::SelfSigned).then(|| certificate_fingerprint(&leaf));
    let config = RustlsConfig::from_pem(certificate, private_key)
        .await
        .map_err(|error| format!("証明書か秘密鍵を読み込めません: {error}"))?;
    Ok(TlsMaterial {
        config,
        certificate: CertificateInfo { expires_at, source },
        fingerprint,
    })
}

/// 置き場の自己署名証明書を読む。無ければ作る。
pub async fn self_signed(dir: &Path) -> Result<TlsMaterial, String> {
    install_crypto_provider();
    let cert_path = dir.join("certificate.pem");
    let key_path = dir.join("private-key.pem");
    let has_cert = cert_path
        .try_exists()
        .map_err(|error| format!("証明書の有無を確かめられません: {error}"))?;
    let has_key = key_path
        .try_exists()
        .map_err(|error| format!("秘密鍵の有無を確かめられません: {error}"))?;
    if has_cert != has_key {
        return Err(format!(
            "サーバーの証明書か秘密鍵が欠けています。{SELF_SIGNED_RECOVERY}"
        ));
    }
    let (certificate, private_key) = if has_cert {
        (
            fs::read(&cert_path).map_err(|error| format!("証明書を読めません: {error}"))?,
            fs::read(&key_path).map_err(|error| format!("秘密鍵を読めません: {error}"))?,
        )
    } else {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["piep.local".into()])
                .map_err(|error| format!("サーバーの証明書を作れません: {error}"))?;
        let pair = (
            cert.pem().into_bytes(),
            signing_key.serialize_pem().into_bytes(),
        );
        fs::create_dir_all(dir).map_err(|error| format!("証明書の置き場を作れません: {error}"))?;
        let key = StagedWrite::new(&key_path, &pair.1)?;
        let cert = StagedWrite::new(&cert_path, &pair.0)?;
        // 両方を同期してから、証明書だけが残らない順で確定する。
        key.commit()?;
        cert.commit()?;
        pair
    };
    material_from_pem(certificate, private_key, CertificateSource::SelfSigned)
        .await
        .map_err(|error| format!("{error}。{SELF_SIGNED_RECOVERY}"))
}

/// 利用者が用意した証明書ファイルを読む。
pub async fn from_files(cert: &Path, key: &Path) -> Result<TlsMaterial, String> {
    install_crypto_provider();
    let certificate = tokio::fs::read(cert)
        .await
        .map_err(|error| format!("証明書ファイルを読めません（{}）: {error}", cert.display()))?;
    let private_key = tokio::fs::read(key)
        .await
        .map_err(|error| format!("秘密鍵ファイルを読めません（{}）: {error}", key.display()))?;
    material_from_pem(certificate, private_key, CertificateSource::Files).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::TestRoot;

    #[tokio::test]
    async fn incomplete_self_signed_files_give_recovery_instructions() {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["piep.local".into()]).unwrap();
        let cert = cert.pem();
        let key = signing_key.serialize_pem();
        for (certificate, private_key) in [
            (None, Some(key.as_bytes())),
            (Some(cert.as_bytes()), None),
            (Some(b"".as_slice()), Some(key.as_bytes())),
            (
                Some(cert.as_bytes()),
                Some(b"-----BEGIN PRIVATE KEY-----\n"),
            ),
        ] {
            let root = TestRoot::created("incomplete-tls");
            for (name, content) in [
                ("certificate.pem", certificate),
                ("private-key.pem", private_key),
            ] {
                if let Some(content) = content {
                    fs::write(root.0.join(name), content).unwrap();
                }
            }
            let error = self_signed(&root.0)
                .await
                .err()
                .expect("壊れた組を読み込めた");
            assert!(
                error.contains("両方削除すると、次の起動で作り直します"),
                "{error}"
            );
            assert!(
                error.contains("登録済みの端末は再登録してください"),
                "{error}"
            );
        }
    }
}
