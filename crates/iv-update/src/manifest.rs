//! 업데이트 피드(`update.json`) — 모든 필드를 정규 문자열로 묶어 Ed25519 서명한다.
//! 검증에 쓴 파싱 결과([`Release`])만 UI·받기에 쓴다 (원문을 다시 읽지 않는다).

use crate::version::Version;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 받을 압축 파일 크기 상한 (지금 앱은 15MB 안팎)
pub const MAX_SIZE: u64 = 300 << 20;
/// 알림 글 상한 (글자 수)
pub const MAX_NOTES: usize = 4000;

/// 피드 원문 — 중복 키·모르는 키는 거부
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub min_os: String,
    pub notes: String,
    pub signature: String,
}

/// 서명이 확인된 새 판
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub min_os: String,
    pub notes: String,
}

/// 서명 대상 — 형식이 바뀌면 머리 글(v1)을 올린다
pub fn signed_message(version: &str, url: &str, size: u64, sha256: &str, min_os: &str, notes: &str) -> String {
    let notes_hash = hex(&Sha256::digest(notes.as_bytes()));
    format!("kaiiv-update-v1\nversion={version}\nurl={url}\nsize={size}\nsha256={sha256}\nmin_os={min_os}\nnotes_sha256={notes_hash}\n")
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn verifying_key(b64: &str) -> Option<VerifyingKey> {
    let b: [u8; 32] = B64.decode(b64.trim()).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&b).ok()
}

fn valid_os(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    (1..=3).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// macOS 판 비교 (`12.0` ≤ `14.6.1`)
pub fn os_at_least(have: &str, need: &str) -> bool {
    let v = |s: &str| -> Vec<u32> { s.split('.').map(|p| p.parse().unwrap_or(0)).chain([0, 0, 0]).take(3).collect() };
    v(have) >= v(need)
}

impl Manifest {
    /// 서명·형식 확인 — 통과해야만 [`Release`]
    pub fn verify(&self, key: &VerifyingKey) -> Result<Release, String> {
        let version = Version::parse(&self.version).ok_or("판 번호 형식이 잘못되었습니다")?;
        // 시험 빌드만 file:// (릴리스는 https 만)
        let scheme_ok = self.url.starts_with("https://") || (cfg!(debug_assertions) && self.url.starts_with("file://"));
        if !scheme_ok || self.url.len() > 512 || self.url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("받을 주소가 잘못되었습니다".into());
        }
        if self.size == 0 || self.size > MAX_SIZE {
            return Err("파일 크기가 잘못되었습니다".into());
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err("해시 형식이 잘못되었습니다".into());
        }
        if !valid_os(&self.min_os) {
            return Err("최소 macOS 판 형식이 잘못되었습니다".into());
        }
        if self.notes.chars().count() > MAX_NOTES {
            return Err("알림 글이 너무 깁니다".into());
        }
        let sig: [u8; 64] = B64.decode(&self.signature).ok().and_then(|b| b.try_into().ok()).ok_or("서명 형식이 잘못되었습니다")?;
        // canonical base64 만 (같은 서명의 다른 표기 거부)
        if B64.encode(sig) != self.signature {
            return Err("서명 형식이 잘못되었습니다".into());
        }
        let msg = signed_message(&self.version, &self.url, self.size, &self.sha256, &self.min_os, &self.notes);
        key.verify(msg.as_bytes(), &Signature::from_bytes(&sig)).map_err(|_| "서명이 맞지 않습니다 — 받지 않습니다")?;
        // 알림 글은 평문으로만 — 제어 문자 제거(줄바꿈·탭은 둔다)
        let notes = self.notes.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect();
        Ok(Release { version, url: self.url.clone(), size: self.size, sha256: self.sha256.clone(), min_os: self.min_os.clone(), notes })
    }

    /// 서명해 피드를 만든다 (배포 도구·시험용)
    pub fn sign(key: &SigningKey, version: &str, url: &str, size: u64, sha256: &str, min_os: &str, notes: &str) -> Self {
        let msg = signed_message(version, url, size, sha256, min_os, notes);
        let signature = B64.encode(key.sign(msg.as_bytes()).to_bytes());
        Manifest { version: version.into(), url: url.into(), size, sha256: sha256.into(), min_os: min_os.into(), notes: notes.into(), signature }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 64 << 10 {
            return Err("피드가 너무 큽니다".into());
        }
        serde_json::from_slice(bytes).map_err(|e| format!("피드를 읽을 수 없습니다: {e}"))
    }
}

#[cfg(test)]
pub(crate) fn test_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(k: &SigningKey) -> Manifest {
        Manifest::sign(k, "0.4.24", "https://example.com/KaiIV-0.4.24.zip", 1234, &"a".repeat(64), "12.0", "새 판\n- 고침")
    }

    #[test]
    fn verifies_and_rejects_tampering() {
        let k = test_key(7);
        let vk = k.verifying_key();
        let m = sample(&k);
        let r = m.verify(&vk).unwrap();
        assert_eq!(r.version, Version(0, 4, 24));
        // 어느 필드든 바꾸면 서명 실패 — 오래된 zip 을 새 판인 척 재생 불가
        let mut t = m.clone();
        t.version = "0.4.99".into();
        assert!(t.verify(&vk).is_err());
        let mut t = m.clone();
        t.url = "https://evil.example/x.zip".into();
        assert!(t.verify(&vk).is_err());
        let mut t = m.clone();
        t.notes.push('!');
        assert!(t.verify(&vk).is_err());
        let mut t = m.clone();
        t.min_os = "99.0".into();
        assert!(t.verify(&vk).is_err());
        // 다른 키
        assert!(m.verify(&test_key(8).verifying_key()).is_err());
    }

    #[test]
    fn rejects_bad_shapes() {
        let k = test_key(1);
        let vk = k.verifying_key();
        let bad = [
            Manifest::sign(&k, "0.4.024", "https://e/x", 1, &"a".repeat(64), "12.0", ""),
            Manifest::sign(&k, "0.4.24", "http://e/x", 1, &"a".repeat(64), "12.0", ""),
            Manifest::sign(&k, "0.4.24", "https://e/x", 0, &"a".repeat(64), "12.0", ""),
            Manifest::sign(&k, "0.4.24", "https://e/x", MAX_SIZE + 1, &"a".repeat(64), "12.0", ""),
            Manifest::sign(&k, "0.4.24", "https://e/x", 1, &"A".repeat(64), "12.0", ""),
            Manifest::sign(&k, "0.4.24", "https://e/x", 1, &"a".repeat(64), "12.x", ""),
        ];
        for m in bad {
            assert!(m.verify(&vk).is_err(), "{m:?}");
        }
    }

    #[test]
    fn json_roundtrip_strict() {
        let k = test_key(3);
        let m = sample(&k);
        let j = serde_json::to_vec(&m).unwrap();
        assert!(Manifest::parse(&j).unwrap().verify(&k.verifying_key()).is_ok());
        assert!(Manifest::parse(br#"{"version":"0.4.24","version":"0.5.0"}"#).is_err());
        let mut v: serde_json::Value = serde_json::from_slice(&j).unwrap();
        v["extra"] = 1.into();
        assert!(Manifest::parse(&serde_json::to_vec(&v).unwrap()).is_err());
    }

    #[test]
    fn os_compare() {
        assert!(os_at_least("14.6.1", "12.0"));
        assert!(os_at_least("12.0", "12"));
        assert!(!os_at_least("11.7", "12.0"));
    }
}
