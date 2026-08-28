//! 持久化设备身份：Ed25519 长期密钥 + 自签名 X.509 证书。
//!
//! 安全边界：
//! - 密钥文件权限 0600（Unix）；Windows 依赖用户目录 ACL（文档如实说明，
//!   DPAPI 包裹列为后续改进）。
//! - 证书自签名，SPKI 为 Ed25519；指纹 = SHA-256(证书 DER)。
//! - 每次进程启动生成新的 instance_id（发现层用于区分重启）。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Ed25519 PKCS#8 前缀（RFC 8410）
const ED25519_PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

pub struct DeviceIdentity {
    pub device_id: String,
    name: std::sync::RwLock<String>,
    pub instance_id: String,
    pub cert_der: Vec<u8>,
    pub fingerprint: String,
    data_dir: PathBuf,
    signing_key: SigningKey,
}

impl DeviceIdentity {
    /// 从数据目录加载或创建身份。name 仅用于展示，可随时修改（改名不换身份）。
    pub fn load_or_create(data_dir: &Path, name: &str) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("无法创建数据目录 {}", data_dir.display()))?;
        let key_path = data_dir.join("identity.key");
        let meta_path = data_dir.join("identity.json");

        let (seed, cert_der, device_id, stored_name) = if key_path.exists() || meta_path.exists() {
            // 尝试加载现有身份；任一文件缺失/损坏时：
            // 先把破坏文件备份成 .corrupt，再重建全新身份（保证可启动）。
            // 身份变化会让旧信任失效，但总比“无法启动”好；原文件保留可排查。
            let seed_hex = std::fs::read_to_string(&key_path).ok();
            let seed = seed_hex
                .and_then(|h| hex_decode(&h).ok())
                .filter(|s| s.len() == 32);
            let meta = std::fs::read_to_string(&meta_path)
                .ok()
                .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok());
            let parsed = match (&meta, seed) {
                (Some(m), Some(s)) => {
                    let device_id = m.get("device_id").and_then(|v| v.as_str());
                    let cert_b64 = m.get("cert").and_then(|v| v.as_str());
                    let cert_der = cert_b64.and_then(|b| {
                        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b).ok()
                    });
                    match (device_id, cert_der) {
                        (Some(dev), Some(cd)) => {
                            let stored_name = m
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("未命名设备")
                                .to_string();
                            let mut arr = [0u8; 32];
                            arr.copy_from_slice(&s[..]);
                            Some((arr, cd, dev.to_string(), stored_name))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            match parsed {
                Some(p) => p,
                None => {
                    tracing::warn!(
                        "身份文件缺失或损坏（key={} meta={}），备份并重建身份；原文件见 identity.json.corrupt / identity.key.corrupt",
                        key_path.exists(),
                        meta_path.exists()
                    );
                    if key_path.exists() {
                        let _ = std::fs::rename(&key_path, data_dir.join("identity.key.corrupt"));
                    }
                    if meta_path.exists() {
                        let _ = std::fs::rename(&meta_path, data_dir.join("identity.json.corrupt"));
                    }
                    let mut seed = [0u8; 32];
                    rand::rngs::OsRng.fill_bytes(&mut seed);
                    let cert_der = Self::make_cert(&seed, name)?;
                    let device_id = uuid::Uuid::new_v4().to_string();
                    let meta = serde_json::json!({
                        "device_id": device_id,
                        "name": name,
                        "cert": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &cert_der),
                    });
                    write_private(&key_path, &hex_encode(&seed))?;
                    write_private(&meta_path, &serde_json::to_string_pretty(&meta)?)?;
                    (seed, cert_der, device_id, name.to_string())
                }
            }
        } else {
            let mut seed = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut seed);
            let cert_der = Self::make_cert(&seed, name)?;
            let device_id = uuid::Uuid::new_v4().to_string();
            let meta = serde_json::json!({
                "device_id": device_id,
                "name": name,
                "cert": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &cert_der),
            });
            write_private(&key_path, &hex_encode(&seed))?;
            write_private(&meta_path, &serde_json::to_string_pretty(&meta)?)?;
            (seed, cert_der, device_id, name.to_string())
        };

        let signing_key = SigningKey::from_bytes(&seed);
        let fingerprint = hex_encode(&Sha256::digest(&cert_der));
        Ok(Self {
            device_id,
            name: std::sync::RwLock::new(stored_name),
            instance_id: uuid::Uuid::new_v4().to_string(),
            cert_der,
            fingerprint,
            data_dir: data_dir.to_path_buf(),
            signing_key,
        })
    }

    /// 生成自签名 Ed25519 证书（CN = device_id，有效期 100 年，长期设备身份）
    fn make_cert(seed: &[u8; 32], name: &str) -> Result<Vec<u8>> {
        let pkcs8 = Self::pkcs8_from_seed(seed);
        let key_der = rustls_pki_types::PrivatePkcs8KeyDer::from(pkcs8);
        let kp = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&key_der, &rcgen::PKCS_ED25519)
            .context("生成密钥对失败")?;
        let mut params =
            rcgen::CertificateParams::new(Vec::<String>::new()).context("证书参数初始化失败")?;
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            format!(
                "lunote-{}",
                name.chars()
                    .filter(|c| *c != ',')
                    .take(64)
                    .collect::<String>()
            ),
        );
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2124, 1, 1);
        let cert = params.self_signed(&kp)?;
        Ok(cert.der().to_vec())
    }

    fn pkcs8_from_seed(seed: &[u8; 32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(48);
        out.extend_from_slice(&ED25519_PKCS8_PREFIX);
        out.extend_from_slice(seed);
        out
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// 对任意数据签名（用于会话挑战）
    pub fn sign(&self, data: &[u8]) -> Signature {
        self.signing_key.sign(data)
    }

    /// 供 TLS 使用的私钥 DER（PKCS#8）
    pub fn tls_key_der(&self) -> rustls_pki_types::PrivateKeyDer<'static> {
        rustls_pki_types::PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(
            Self::pkcs8_from_seed(&self.signing_key.to_bytes()),
        ))
    }

    /// 当前显示名称（可随时修改，不改变身份与密钥）
    pub fn name(&self) -> String {
        self.name.read().unwrap().clone()
    }

    /// 更新显示名称（持久化；不改变身份与密钥）
    pub fn set_name(&self, name: &str) -> Result<()> {
        let meta = serde_json::json!({
            "device_id": self.device_id,
            "name": name,
            "cert": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &self.cert_der),
        });
        write_private(
            &self.data_dir.join("identity.json"),
            &serde_json::to_string_pretty(&meta)?,
        )?;
        *self.name.write().unwrap() = name.to_string();
        Ok(())
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

/// 从证书 DER 提取 Ed25519 公钥（仅接受 Ed25519 自签名证书，OID 1.3.101.112）
pub fn extract_ed25519_pubkey(cert_der: &[u8]) -> Option<[u8; 32]> {
    use x509_parser::prelude::*;
    let (_, cert) = X509Certificate::from_der(cert_der).ok()?;
    let spki = cert.public_key();
    if spki.algorithm.algorithm.to_id_string() != "1.3.101.112" {
        return None;
    }
    let data = &spki.subject_public_key.data;
    if data.len() != 32 {
        return None;
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(data);
    Some(key)
}

/// 写入私有文件（原子：临时文件 + 改名；Unix 0600；Windows 依赖目录 ACL）。
/// 非原子直接覆盖可能在进程被杀时留下损坏文件，导致下次启动身份加载失败。
fn write_private(path: &Path, content: &str) -> Result<()> {
    crate::platform::atomic_write(path, content.as_bytes(), true)
        .with_context(|| format!("落盘 {} 失败", path.display()))
}

fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("十六进制长度必须为偶数");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| anyhow!("非法十六进制字符")))
        .collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex_encode(&Sha256::digest(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_stable_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        let a = DeviceIdentity::load_or_create(dir.path(), "测试机").unwrap();
        let b = DeviceIdentity::load_or_create(dir.path(), "改名").unwrap();
        assert_eq!(a.device_id, b.device_id);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.cert_der, b.cert_der);
        assert_eq!(a.name(), "测试机"); // 加载时保留旧名
    }

    #[test]
    fn key_signs_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let id = DeviceIdentity::load_or_create(dir.path(), "X").unwrap();
        let sig = id.sign(b"challenge-data");
        id.verifying_key()
            .verify_strict(b"challenge-data", &sig)
            .unwrap();
        assert!(id.verifying_key().verify_strict(b"other", &sig).is_err());
    }

    #[test]
    fn pubkey_extraction_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let id = DeviceIdentity::load_or_create(dir.path(), "X").unwrap();
        let key = extract_ed25519_pubkey(&id.cert_der).unwrap();
        assert_eq!(key, id.verifying_key().to_bytes());
    }

    #[test]
    fn garbage_cert_rejected() {
        assert!(extract_ed25519_pubkey(b"not-a-cert").is_none());
    }

    #[test]
    fn renamed_identity_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let id = DeviceIdentity::load_or_create(dir.path(), "旧名称").unwrap();
        id.set_name("新名称").unwrap();
        let reloaded = DeviceIdentity::load_or_create(dir.path(), "默认名称").unwrap();
        assert_eq!(reloaded.name(), "新名称");
        assert_eq!(reloaded.device_id, id.device_id);
    }
}
