//! 인앱 업데이트 조립 — iv-update(확인·받기·검사) 상태를 창·상태줄에 잇는다.
//!
//! 자동 확인: 시작 5초 뒤, 하루 1회(끄기 가능). 수동: 상태줄 판 번호 · 파일 메뉴 "업데이트 확인…".
//! 자가 시험은 확인하지 않는다 — 디버그 빌드에서 `IV_UPDATE_FEED`(file://) 를 줄 때만 시험 피드로.

use crate::window_app::UserEvent;
use iv_update::{Status, Updater, Version};
use iv_viewer::Action;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

/// 업데이트 창 그리기용 (빌린 문자열로 바꿔 iv-ui 에 넘긴다)
pub struct UpdateOwned {
    current: String,
    auto: bool,
    kind: u8,
    version: String,
    notes: String,
    message: String,
    got: u64,
    total: u64,
}

impl UpdateOwned {
    pub fn as_ui(&self) -> iv_ui::UpdateUi<'_> {
        use iv_ui::UpdateView as V;
        let view = match self.kind {
            0 => V::Checking,
            1 => V::UpToDate,
            2 => V::Available { version: &self.version, notes: &self.notes, blocked: (!self.message.is_empty()).then_some(self.message.as_str()) },
            3 => V::Downloading { version: &self.version, got: self.got, total: self.total },
            4 => V::Ready { version: &self.version },
            _ => V::Failed { message: &self.message },
        };
        iv_ui::UpdateUi { current: &self.current, view, auto: self.auto }
    }
}

/// 다시 시작해 설치할 때 앱이 할 일
pub struct Restart {
    pub staged: iv_update::Staged,
    pub target: PathBuf,
    pub log: PathBuf,
}

pub struct AppUpdate {
    updater: Option<Updater>,
    state: iv_update::State,
    state_path: Option<PathBuf>,
    /// 창을 띄울까
    pub prompt: bool,
    /// 이번 실행에서 '나중에' 한 판 (수동 확인 전까진 다시 띄우지 않음)
    dismissed: Option<Version>,
    /// 자동 설치를 못 하는 까닭 (새 판이 왔을 때 계산)
    blocked: Option<String>,
    target: Option<PathBuf>,
    auto_at: Option<Instant>,
    selftest: bool,
}

impl AppUpdate {
    pub fn new(proxy: EventLoopProxy<UserEvent>, selftest: bool) -> Self {
        let test_feed = cfg!(debug_assertions) && std::env::var_os("IV_UPDATE_FEED").is_some();
        let enabled = std::env::var_os("IV_UPDATE_NO_CHECK").is_none() && (!selftest || test_feed);
        let work = iv_thumbs::default_cache_dir().map(|d| d.join("update"));
        let updater = enabled
            .then(|| iv_update::Config::new(env!("CARGO_PKG_VERSION"), work?))
            .flatten()
            .map(|cfg| Updater::new(cfg, move || {
                let _ = proxy.send_event(UserEvent::UpdateWake);
            }));
        // 시험은 사용자 설정 파일을 쓰지 않는다
        let state_path = if cfg!(debug_assertions) && std::env::var_os("IV_UPDATE_STATE").is_some() {
            std::env::var_os("IV_UPDATE_STATE").map(PathBuf::from)
        } else if selftest {
            None
        } else {
            iv_update::State::default_path()
        };
        let state = state_path.as_deref().map(iv_update::State::load).unwrap_or_default();
        let auto_at = (updater.is_some() && (state.due(iv_update::now_secs()) || test_feed)).then(|| Instant::now() + Duration::from_secs(if test_feed { 1 } else { 5 }));
        // 지난 업데이트가 남긴 단계 폴더 치우기
        if updater.is_some() && !selftest {
            let _ = std::thread::Builder::new().name("iv-update-cleanup".into()).spawn(|| {
                if let Ok(t) = iv_update::install_target() {
                    iv_update::install::cleanup_stale(&t);
                }
            });
        }
        if updater.is_none() {
            log::info!("업데이트 확인 꺼짐 (자가 시험·IV_UPDATE_NO_CHECK·설정 없음)");
        }
        Self { updater, state, state_path, prompt: false, dismissed: None, blocked: None, target: None, auto_at, selftest }
    }

    fn save(&self) {
        if let Some(p) = &self.state_path {
            if let Err(e) = self.state.save(p) {
                log::warn!("업데이트 설정 저장 실패: {e}");
            }
        }
    }

    fn status(&self) -> Status {
        self.updater.as_ref().map_or(Status::Idle, Updater::status)
    }

    /// about_to_wait 에서 — 자동 확인 시각이면 확인. 다음 깨울 시각.
    pub fn tick(&mut self) -> Option<Instant> {
        let at = self.auto_at?;
        if Instant::now() < at {
            return Some(at);
        }
        self.auto_at = None;
        if let Some(u) = &self.updater {
            u.check(false, self.state.skip, self.state.highest);
            self.state.last_check = iv_update::now_secs();
            self.save();
        }
        None
    }

    /// 백그라운드 결과가 왔다
    pub fn on_wake(&mut self) {
        match self.status() {
            Status::Available(r) => {
                if self.state.highest.is_none_or(|h| r.version > h) {
                    self.state.highest = Some(r.version);
                    self.save();
                }
                if self.blocked.is_none() && self.target.is_none() {
                    match iv_update::install_target() {
                        Ok(t) => self.target = Some(t),
                        Err(e) => self.blocked = Some(e),
                    }
                }
                if self.dismissed != Some(r.version) {
                    self.prompt = true;
                }
            }
            Status::Ready { .. } | Status::UpToDate => self.prompt = true,
            Status::Failed { manual: true, .. } => self.prompt = true,
            _ => {}
        }
    }

    /// 상태줄 강조 — 받을 새 판
    pub fn badge(&self) -> Option<String> {
        match self.status() {
            Status::Available(r) | Status::Downloading { release: r, .. } | Status::Ready { release: r, .. } => Some(r.version.to_string()),
            _ => None,
        }
    }

    pub fn ui(&self) -> Option<UpdateOwned> {
        if !self.prompt {
            return None;
        }
        let mut o = UpdateOwned { current: env!("CARGO_PKG_VERSION").into(), auto: self.state.auto, kind: 5, version: String::new(), notes: String::new(), message: String::new(), got: 0, total: 0 };
        match self.status() {
            Status::Idle | Status::Checking { .. } => o.kind = 0,
            Status::UpToDate => o.kind = 1,
            Status::Available(r) => {
                o.kind = 2;
                o.version = r.version.to_string();
                o.notes = r.notes;
                o.message = self.blocked.clone().unwrap_or_default();
            }
            Status::Downloading { release, got } => {
                o.kind = 3;
                o.version = release.version.to_string();
                o.got = got;
                o.total = release.size;
            }
            Status::Ready { release, .. } => {
                o.kind = 4;
                o.version = release.version.to_string();
            }
            Status::Failed { message, .. } => o.message = message,
        }
        if self.updater.is_none() {
            o.kind = 5;
            o.message = "이 실행에서는 업데이트 확인이 꺼져 있습니다".into();
        }
        Some(o)
    }

    /// 업데이트 Action 처리. 다시 시작해 설치면 [`Restart`].
    pub fn handle(&mut self, a: &Action) -> Option<Restart> {
        let st = self.status();
        match a {
            Action::UpdateCheck => {
                self.prompt = true;
                if let Some(u) = &self.updater
                    && !matches!(st, Status::Available(_) | Status::Downloading { .. } | Status::Ready { .. })
                {
                    u.check(true, self.state.skip, self.state.highest);
                    self.state.last_check = iv_update::now_secs();
                    self.save();
                }
            }
            Action::UpdateNow => {
                if let (Status::Available(r), Some(u), Some(t)) = (st, &self.updater, &self.target) {
                    u.download(r, t.clone());
                }
            }
            Action::UpdateLater => {
                self.prompt = false;
                match st {
                    Status::Available(r) => self.dismissed = Some(r.version),
                    Status::UpToDate | Status::Failed { .. } | Status::Checking { .. } => {
                        if let Some(u) = &self.updater {
                            u.set_idle();
                        }
                    }
                    _ => {}
                }
            }
            Action::UpdateSkip => {
                if let Status::Available(r) = st {
                    self.state.skip = Some(r.version);
                    self.save();
                }
                self.prompt = false;
                if let Some(u) = &self.updater {
                    u.set_idle();
                }
            }
            Action::UpdateOpenPage => {
                self.prompt = false;
                if self.selftest {
                    log::info!("자가 시험: 배포 쪽 열기 생략");
                } else if let Err(e) = std::process::Command::new("/usr/bin/open").arg(iv_update::RELEASES_PAGE).spawn() {
                    log::warn!("배포 쪽 열기 실패: {e}");
                }
            }
            Action::SetUpdateAuto(on) => {
                self.state.auto = *on;
                self.save();
            }
            Action::UpdateRestart => {
                if let (Status::Ready { staged, .. }, Some(t), Some(u)) = (st, &self.target, &self.updater) {
                    // 준비해 둔 것이 사라졌으면(다른 실행이 치움 등) 다시 받게
                    if !staged.dir.join("new.app").is_dir() {
                        log::warn!("준비해 둔 업데이트가 사라짐 — 다시 받기로");
                        u.forget_staged();
                        return None;
                    }
                    return Some(Restart { staged, target: t.clone(), log: u.config().work_dir.join("update.log") });
                }
            }
            _ => {}
        }
        None
    }
}
