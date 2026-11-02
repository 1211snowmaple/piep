//! 待ち受けの TLS。自己署名の証明書は作って置き場に残し、指紋を登録文字列に載せる。

use axum_server::tls_rustls::RustlsConfig;
use rcgen::{generate_simple_self_signed, CertifiedKey};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufReader, Cursor};
use std::path::Path;

pub struct TlsMaterial {
    pub config: RustlsConfig,
    /// 自己署名のときだけ。OS が信頼する CA の証明書はホスト名で検証する。
    pub fingerprint: Option<String>,
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

fn pem_fingerprint(certificate: &[u8]) -> Result<String, String> {
    let mut reader = BufReader::new(Cursor::new(certificate));
    let first = rustls_pemfile::certs(&mut reader)
        .next()
        .ok_or("証明書が空です")?
        .map_err(|error| format!("証明書を解析できません: {error}"))?;
    Ok(certificate_fingerprint(first.as_ref()))
}

/// 置き場の自己署名証明書を読む。無ければ作る。
pub async fn self_signed(dir: &Path) -> Result<TlsMaterial, String> {
    install_crypto_provider();
    let cert_path = dir.join("certificate.pem");
    let key_path = dir.join("private-key.pem");
    if cert_path.exists() != key_path.exists() {
        return Err("サーバーの証明書か秘密鍵が欠けています。登録済みの端末がつながらなくなるので、自動では作り直しません".into());
    }
    let (certificate, private_key) = if cert_path.exists() {
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
        fs::write(&cert_path, &pair.0)
            .map_err(|error| format!("証明書を保存できません: {error}"))?;
        fs::write(&key_path, &pair.1)
            .map_err(|error| format!("秘密鍵を保存できません: {error}"))?;
        pair
    };
    let fingerprint = pem_fingerprint(&certificate)?;
    let config = RustlsConfig::from_pem(certificate, private_key)
        .await
        .map_err(|error| format!("証明書を読み込めません: {error}"))?;
    Ok(TlsMaterial {
        config,
        fingerprint: Some(fingerprint),
    })
}

/// 利用者が用意した証明書ファイルを読む。
pub async fn from_files(cert: &Path, key: &Path) -> Result<TlsMaterial, String> {
    install_crypto_provider();
    let config = RustlsConfig::from_pem_file(cert, key)
        .await
        .map_err(|error| format!("証明書ファイルを読み込めません: {error}"))?;
    Ok(TlsMaterial {
        config,
        fingerprint: None,
    })
}
