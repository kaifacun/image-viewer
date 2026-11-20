//! 추가 코덱 엔진 — FFmpeg 를 **별도 프로그램**으로 찾고, 없으면 설치한다 (앱 안에 올리지 않는다:
//! 엔진이 죽어도 앱은 살고, 판이 바뀌어도 앱이 깨지지 않고, 라이선스가 깔끔하다).
//!
//! 찾는 순서: KaiIV 코덱 폴더(우리가 검증해 넣은 것) → Homebrew(/opt/homebrew, /usr/local) → PATH.
//! 설치 순서: Homebrew 가 있으면 `brew install ffmpeg`, 없으면 고정된 빌드를 받아 **SHA-256 이 맞을 때만** 쓴다.
//! 사용자가 한 번 동의하면 기억하고([`set_consent`]) 이후는 자동.
//!
//! 시험용: `IV_CODEC_DIR` = 코덱 폴더, `IV_CODEC_NO_INSTALL` = 설치 금지(자가 시험), `IV_CODEC_NO_SYSTEM` = Homebrew·PATH 무시.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// 찾은 엔진
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// 어디서 왔나 (상태 표시)
    pub source: &'static str,
}

//#region 고정 빌드 — 배포처(ffmpeg.martin-riedl.de, ffmpeg.org 가 안내하는 macOS 빌드)의 9.0.2 arm64, 받은 뒤 SHA-256 대조
struct Pinned {
    name: &'static str,
    url: &'static str,
    sha256: &'static str,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const PINNED: &[Pinned] = &[
    Pinned {
        name: "ffmpeg",
        url: "https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip",
        sha256: "c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924",
    },
    Pinned {
        name: "ffprobe",
        url: "https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffprobe.zip",
        sha256: "fcbe839537485eaee7a7a8bc5cbc0f90d53617e80943e8a5b2e31cb851197ea6",
    },
];

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
const PINNED: &[Pinned] = &[];
//#endregion

/// KaiIV 가 관리하는 코덱 폴더 (~/Library/Application Support/KaiIV/codecs)
pub fn codec_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("IV_CODEC_DIR") {
        return Some(d.into());
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/KaiIV/codecs"))
}

fn executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// 엔진 찾기 (파일 존재만 확인 — 가볍다, 매번 불러도 된다)
pub fn find() -> Option<Tools> {
    let pair = |dir: &Path, source| {
        let (f, p) = (dir.join("ffmpeg"), dir.join("ffprobe"));
        (executable(&f) && executable(&p)).then(|| Tools { ffmpeg: f, ffprobe: p, source })
    };
    if let Some(t) = codec_dir().and_then(|d| pair(&d, "KaiIV 코덱 팩")) {
        return Some(t);
    }
    if std::env::var_os("IV_CODEC_NO_SYSTEM").is_some() {
        return None;
    }
    for d in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if let Some(t) = pair(Path::new(d), "Homebrew") {
            return Some(t);
        }
    }
    std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).find_map(|d| pair(&d, "PATH")))
}

//#region 동의 — 한 번 받으면 이후 자동
fn consent_file() -> Option<PathBuf> {
    codec_dir().map(|d| d.with_file_name("codec-consent"))
}

pub fn consented() -> bool {
    consent_file().is_some_and(|f| f.exists())
}

pub fn set_consent() {
    if let Some(f) = consent_file() {
        let _ = f.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(f, b"KaiIV: user agreed to install the FFmpeg codec engine\n");
    }
}
//#endregion

//#region 설치 — 백그라운드 스레드, 진행 상황은 [`Installer::status`]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallStatus {
    Running(String),
    Done(Tools),
    Failed(String),
}

pub struct Installer {
    status: Arc<Mutex<InstallStatus>>,
}

impl Installer {
    /// 설치를 시작한다. `wake` 는 상태가 바뀔 때 불린다.
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let status = Arc::new(Mutex::new(InstallStatus::Running("코덱 엔진 준비 중…".into())));
        let s = status.clone();
        let set = move |st: InstallStatus| {
            *s.lock().unwrap_or_else(|e| e.into_inner()) = st;
            wake();
        };
        std::thread::Builder::new()
            .name("iv-codec-install".into())
            .spawn(move || {
                let r = install(&|msg: &str| set(InstallStatus::Running(msg.to_string())));
                set(match r {
                    Ok(t) => InstallStatus::Done(t),
                    Err(e) => InstallStatus::Failed(e),
                });
            })
            .ok();
        Self { status }
    }

    pub fn status(&self) -> InstallStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn install(progress: &dyn Fn(&str)) -> Result<Tools, String> {
    if std::env::var_os("IV_CODEC_NO_INSTALL").is_some() {
        return Err("설치 금지 (자가 시험)".into());
    }
    if let Some(t) = find() {
        return Ok(t);
    }
    if std::env::var_os("IV_CODEC_NO_SYSTEM").is_none() {
        if let Some(brew) = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].into_iter().map(Path::new).find(|p| executable(p)) {
            progress("Homebrew 로 FFmpeg 설치 중… (몇 분 걸릴 수 있음)");
            log::info!("코덱: brew install ffmpeg");
            let out = Command::new(brew).args(["install", "ffmpeg"]).env("HOMEBREW_NO_AUTO_UPDATE", "1").env("HOMEBREW_NO_ENV_HINTS", "1").output().map_err(|e| e.to_string())?;
            if let Some(t) = find() {
                return Ok(t);
            }
            log::warn!("brew 설치 실패: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""));
        }
    }
    download_pinned(progress)
}

/// 고정 빌드 받기 → SHA-256 대조 → 풀기 → 코덱 폴더에 넣기. 하나라도 맞지 않으면 아무것도 넣지 않는다.
fn download_pinned(progress: &dyn Fn(&str)) -> Result<Tools, String> {
    if PINNED.is_empty() {
        return Err("이 기기용 코덱 엔진 빌드가 없음 — Homebrew 로 ffmpeg 를 설치하세요".into());
    }
    let dir = codec_dir().ok_or("코덱 폴더를 정할 수 없음")?;
    let work = dir.with_file_name("codecs.download");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        for (i, p) in PINNED.iter().enumerate() {
            progress(&format!("코덱 엔진 받는 중… ({}/{}) {}", i + 1, PINNED.len(), p.name));
            let zip = work.join(format!("{}.zip", p.name));
            let st = Command::new("/usr/bin/curl").args(["-fsSL", "--proto", "=https", "--max-time", "600", "-A", "KaiIV", "-o"]).arg(&zip).arg(p.url).status().map_err(|e| e.to_string())?;
            if !st.success() {
                return Err(format!("{} 받기 실패", p.name));
            }
            let got = sha256(&zip)?;
            if got != p.sha256 {
                return Err(format!("{} 검증 실패 (SHA-256 이 다름) — 설치하지 않음", p.name));
            }
            let st = Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&zip).arg(&work).status().map_err(|e| e.to_string())?;
            if !st.success() || !work.join(p.name).is_file() {
                return Err(format!("{} 풀기 실패", p.name));
            }
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(work.join(p.name), std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        // 다 맞을 때만 한 번에 넣는다
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for p in PINNED {
            std::fs::rename(work.join(p.name), dir.join(p.name)).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result?;
    find().ok_or_else(|| "설치했지만 찾지 못함".into())
}

fn sha256(p: &Path) -> Result<String, String> {
    let out = Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(p).output().map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out.stdout).split_whitespace().next().map(str::to_string).ok_or_else(|| "SHA-256 계산 실패".into())
}
//#endregion

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_known_bytes() {
        let f = std::env::temp_dir().join(format!("iv-codec-sha-{}", std::process::id()));
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(sha256(&f).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_file(f);
    }

    /// 실제로 고정 빌드를 받아 검증·설치까지 (실행은 하지 않는다) — 네트워크가 필요해 수동으로만:
    /// IV_CODEC_DIR=<빈 폴더> cargo test -p iv-codecpack -- --ignored
    #[test]
    #[ignore]
    fn downloads_and_verifies_pinned_build() {
        assert!(std::env::var_os("IV_CODEC_DIR").is_some(), "IV_CODEC_DIR 를 시험 폴더로");
        let t = download_pinned(&|m| eprintln!("{m}")).unwrap();
        assert_eq!(t.source, "KaiIV 코덱 팩");
        assert!(executable(&t.ffmpeg) && executable(&t.ffprobe));
    }

    #[test]
    fn pinned_hashes_look_valid() {
        for p in PINNED {
            assert_eq!(p.sha256.len(), 64);
            assert!(p.url.starts_with("https://"));
        }
    }
}
