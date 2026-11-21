//! KaiIV 인앱 업데이트 (Sparkle 방식).
//!
//! - 피드 `update.json` 은 Ed25519 로 서명 — 공개키는 릴리스 바이너리 상수 [`PUBLIC_KEY`], 비밀키는 배포자 키체인에만.
//! - ad-hoc 코드 서명은 무결성만 보장하므로 신원 확인은 Ed25519 하나뿐 → 릴리스 빌드는 피드 주소·키를 바꿀 수 없다.
//! - 앱은 [`Updater`] 로 확인·받기·풀기를 백그라운드에서 하고, 다시 시작할 때 [`install::spawn_apply`] 후 종료.

pub mod install;
pub mod manifest;
mod state;
pub mod version;

pub use install::{APPLY_FLAG, Staged, install_target, run_apply};
pub use manifest::{Manifest, Release};
pub use state::{State, now_secs};
pub use version::Version;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 배포 서명 공개키 (base64, Ed25519) — `kaiiv-update-sign pubkey` 로 얻는다
pub const PUBLIC_KEY: &str = "fmJk0fbe74aEb03Fe3fv4j3grW+UjG80kGonRqvuOOU=";
pub const FEED_URL: &str = "https://github.com/kaifacun/image-viewer/releases/latest/download/update.json";
pub const RELEASES_PAGE: &str = "https://github.com/kaifacun/image-viewer/releases/latest";

#[derive(Clone)]
pub struct Config {
    pub feed_url: String,
    pub key: ed25519_dalek::VerifyingKey,
    pub current: Version,
    /// 받은 압축 파일·로그 (~/Library/Caches/KaiIV/update)
    pub work_dir: PathBuf,
}

impl Config {
    /// 릴리스 빌드: 상수만. 디버그 빌드: `IV_UPDATE_FEED`·`IV_UPDATE_PUBKEY` 로 시험 피드.
    pub fn new(current: &str, work_dir: PathBuf) -> Option<Self> {
        let current = Version::parse(current)?;
        #[allow(unused_mut)]
        let (mut feed, mut key) = (FEED_URL.to_string(), PUBLIC_KEY.to_string());
        #[cfg(debug_assertions)]
        {
            if let Ok(f) = std::env::var("IV_UPDATE_FEED") {
                feed = f;
            }
            if let Ok(k) = std::env::var("IV_UPDATE_PUBKEY") {
                key = k;
            }
        }
        Some(Config { feed_url: feed, key: manifest::verifying_key(&key)?, current, work_dir })
    }
}

/// 지금 macOS 판 (`sw_vers`)
pub fn os_version() -> String {
    std::process::Command::new("/usr/bin/sw_vers").arg("-productVersion").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Checking { manual: bool },
    UpToDate,
    Available(Release),
    Downloading { release: Release, got: u64 },
    Ready { release: Release, staged: Staged },
    Failed { message: String, manual: bool },
}

pub struct Updater {
    cfg: Config,
    status: Arc<Mutex<Status>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Updater {
    pub fn new(cfg: Config, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self { cfg, status: Arc::new(Mutex::new(Status::Idle)), wake: Arc::new(wake) }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_idle(&self) {
        self.set(Status::Idle);
    }

    fn set(&self, s: Status) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = s;
        (self.wake)();
    }

    fn busy(&self) -> bool {
        matches!(self.status(), Status::Checking { .. } | Status::Downloading { .. })
    }

    /// 피드 확인. `manual` 이면 '최신' 과 오류도 보여 줄 상태로. `skip` 은 건너뛰기로 한 판,
    /// `floor` 는 지금까지 본 가장 높은 판 (그보다 낮으면 철회된 판의 재생으로 보고 무시).
    pub fn check(&self, manual: bool, skip: Option<Version>, floor: Option<Version>) {
        if self.busy() {
            return;
        }
        self.set(Status::Checking { manual });
        let (cfg, status, wake) = (self.cfg.clone(), self.status.clone(), self.wake.clone());
        let _ = std::thread::Builder::new().name("iv-update-check".into()).spawn(move || {
            let r = install::fetch_feed(&cfg.feed_url).and_then(|b| Manifest::parse(&b)).and_then(|m| m.verify(&cfg.key));
            let next = match r {
                Ok(rel) if rel.version <= cfg.current => Status::UpToDate,
                Ok(rel) if floor.is_some_and(|f| rel.version < f) => {
                    log::warn!("이미 본 판({floor:?})보다 낮은 서명 판 {} — 무시", rel.version);
                    Status::UpToDate
                }
                Ok(rel) if !manifest::os_at_least(&os_version(), &rel.min_os) => {
                    Status::Failed { message: format!("새 판 {} 은 macOS {} 이상이 필요합니다", rel.version, rel.min_os), manual }
                }
                Ok(rel) if !manual && Some(rel.version) == skip => Status::Idle,
                Ok(rel) => Status::Available(rel),
                Err(e) => {
                    log::info!("업데이트 확인 실패: {e}");
                    Status::Failed { message: e, manual }
                }
            };
            // 백그라운드 확인의 '최신'·실패는 조용히
            let next = match next {
                Status::UpToDate | Status::Failed { .. } if !manual => Status::Idle,
                s => s,
            };
            *status.lock().unwrap_or_else(|e| e.into_inner()) = next;
            wake();
        });
    }

    /// 준비해 둔 단계 폴더가 사라졌을 때 (다른 KaiIV 실행이 치움 등) — 다시 받을 수 있게
    pub fn forget_staged(&self) {
        if let Status::Ready { release, .. } = self.status() {
            self.set(Status::Available(release));
        }
    }

    /// 받고 풀어 검사까지. 끝나면 [`Status::Ready`].
    pub fn download(&self, release: Release, target: PathBuf) {
        if self.busy() {
            return;
        }
        self.set(Status::Downloading { release: release.clone(), got: 0 });
        let (cfg, status, wake) = (self.cfg.clone(), self.status.clone(), self.wake.clone());
        let _ = std::thread::Builder::new().name("iv-update-download".into()).spawn(move || {
            let progress = |got: u64| {
                let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                if let Status::Downloading { got: g, .. } = &mut *s
                    && *g != got
                {
                    *g = got;
                    drop(s);
                    wake();
                }
            };
            let r = install::download(&release, &cfg.work_dir, &progress).and_then(|zip| {
                let st = install::stage(&zip, &release, &target);
                let _ = std::fs::remove_file(&zip);
                st
            });
            let next = match r {
                Ok(staged) => Status::Ready { release, staged },
                Err(message) => Status::Failed { message, manual: true },
            };
            *status.lock().unwrap_or_else(|e| e.into_inner()) = next;
            wake();
        });
    }
}
