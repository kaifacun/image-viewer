//! winit 이벤트 루프 — 입력을 Action 으로, 파이프라인·썸네일 결과를 뷰어로, 장면을 렌더러로 잇는다.
//!
//! 두 화면: 브라우저(트리·격자·미리보기) ↔ 전체 보기. 모드는 `ViewerState::mode` 하나가 진실이다.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iv_core::DecoderRegistry;
use iv_fstree::FolderTree;
use iv_library::EntryId;
use iv_pipeline::{Loader, LoaderConfig};
use iv_render::{EguiPaint, RenderOutcome, Renderer, Scene, UiCapability};
use iv_core::formats::MediaKind;
use iv_scan::{ScanConfig, Scanner, Verdict};
use iv_thumbs::{ThumbConfig, ThumbEvent, ThumbRequest, ThumbService};
use iv_ui::browser::{BrowserModel, BrowserState};
use iv_viewer::{Action, AppRequest, ClipOp, Effects, FileOp, Mode, PageCmd, VideoCmd, ViewerConfig, ViewerState};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Fullscreen, Window, WindowId};

use crate::args::{Args, RendererLevel};
use crate::selftest::{SelfTest, Step};
use crate::{input, renderer};

/// 창 제목·정보 창에 보이는 앱 이름과 버전 (워크스페이스 version 하나가 진실)
const APP: &str = concat!("KaiIV ", env!("CARGO_PKG_VERSION"));

/// 백그라운드 → 메인 스레드 신호
#[derive(Debug, Clone, Copy)]
pub enum UserEvent {
    /// 파이프라인에 결과가 쌓였다
    LoaderWake,
    /// 썸네일 서비스에 결과가 쌓였다
    ThumbWake,
    /// 백그라운드 스캔이 진행됐다 (트리 숨김·상태줄 갱신)
    ScanWake,
    /// 코덱 엔진 설치·영상 변환이 진행됐다
    CodecWake,
    /// 문서 쪽 그리기가 끝났다
    DocWake,
}

//#region PDF 전체 보기 — 쪽 상태와 백그라운드 쪽 그리기
struct PdfView {
    key: u64,
    path: PathBuf,
    pages: usize,
    page: usize,
    /// 지금 화면 쪽을 그린 긴 변 (확대하면 더 크게 다시)
    rendered: u32,
    /// 그리는 중: (쪽, 긴 변, 결과 받는 곳)
    job: Option<(usize, u32, std::sync::mpsc::Receiver<Result<iv_core::Frame, String>>)>,
}
//#endregion

const BACKGROUND: [u8; 3] = [24, 24, 26];

pub fn run(args: Args, registry: DecoderRegistry) {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().expect("이벤트 루프 생성 실패");
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(args, registry, proxy);
    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("이벤트 루프 오류: {e}");
    }
}

struct Gfx {
    window: Arc<Window>,
    renderer: Box<dyn Renderer>,
    level: RendererLevel,
    egui: Option<(egui::Context, egui_winit::State)>,
}

struct App {
    args: Args,
    loader: Loader,
    thumbs: ThumbService,
    viewer: ViewerState,
    tree: FolderTree,
    /// 백그라운드 사진 폴더 스캔 (트리에서 사진 없는 폴더 숨김)
    scanner: Scanner,
    wait_scan_until: Option<Instant>,
    /// 트리 줄 캐시 — 스캔 판정이 바뀌어도 초당 한 번, 마우스를 누르고 있지 않을 때만 다시 만든다 (클릭 중 줄이 밀리지 않게)
    tree_rows: Vec<iv_fstree::Row>,
    tree_counts: Vec<Option<(u32, u32)>>,
    tree_rows_at: Option<Instant>,
    /// 사용자가 트리를 바꿈 (펼침·폴더 이동) — 바로 다시 만든다
    tree_dirty: bool,
    /// 사용자가 만들거나 이름을 바꾼 폴더 — 비어 있어도 숨기지 않는다
    keep_visible: HashSet<PathBuf>,
    /// 잘라낸 파일들과 그때의 클립보드 변경 번호 — 붙여넣을 때 클립보드가 그대로면 옮기고, 아니면 복사
    cut_paths: HashSet<PathBuf>,
    cut_count: i64,
    /// 형식 묶음 (확장자 필터 메뉴·스캔 비트)
    groups: Vec<iv_core::formats::FormatGroup>,
    /// 스캔 판정이 바뀜 — 다음 기회(초당 한 번, 누르고 있지 않을 때)에 다시 만든다
    tree_scan_changed: bool,
    /// 전체 보기 중 열린 폴더가 밖에서 바뀜 — 브라우저로 돌아오면 확인 (전체 보기 확대 상태를 깨지 않게)
    dir_dirty: bool,
    browser: BrowserState,
    /// 썸네일 생성에 실패한 항목 (타일 경고·재요청 제외)
    thumb_failed: HashSet<u64>,
    /// 직전에 썸네일 서비스에 보낸 키 목록 — 달라질 때만 다시 보낸다
    last_requested: Vec<u64>,
    /// 썸네일 텍스처가 속한 폴더 (바뀌면 비움)
    textures_dir: Option<PathBuf>,
    /// 미리보기 칸 (물리 px, 직전 egui 레이아웃)
    preview_area: Option<[f32; 4]>,
    gfx: Option<Gfx>,
    mods: ModifiersState,
    cursor: PhysicalPosition<f64>,
    dragging: Option<PhysicalPosition<f64>>,
    last_click: Option<Instant>,
    /// egui 가 요청한 다음 다시 그리기 시각
    repaint_at: Option<Instant>,
    info_rows: Vec<(String, String)>,
    pending_open: Option<PathBuf>,
    selftest: Option<SelfTest>,
    /// 재생 중인 비디오 (항목 키, 재생기) — 전체 보기·비디오 모드의 현재 항목만
    video: Option<(u64, iv_video::VideoPlayer)>,
    /// 일시 정지 중에도 이때까지는 프레임을 꺼낸다 (위치 옮기기 직후 화면이 따라오게)
    video_poll_until: Option<Instant>,
    //#region 추가 코덱 엔진 (FFmpeg) — 설치·변환
    proxy: EventLoopProxy<UserEvent>,
    codec_installer: Option<iv_codecpack::Installer>,
    /// 엔진 받기 동의를 묻는 창
    codec_prompt: bool,
    /// 이번 실행에서 "나중에" 를 눌렀다 (다시 묻지 않는다)
    codec_declined: bool,
    /// 재생용 변환 작업 (항목 키, 작업)
    convert: Option<(u64, iv_ffmpeg::Job)>,
    /// 기본 엔진으로 못 연 항목 — 변환 경로로
    needs_convert: HashSet<u64>,
    /// 전체 보기 중인 PDF
    pdf: Option<PdfView>,
    /// 전체 보기 중인 글 문서 (항목 키, 내용)
    text_doc: Option<(u64, crate::textdoc::TextDoc)>,
    /// 마크다운을 꾸며 보이기 (끄면 원문) — 파일을 넘겨도 유지
    md_rendered: bool,
    //#endregion
    /// 자가 시험이 주입할 egui 입력 (프레임마다 한 묶음)
    injected: VecDeque<Vec<egui::Event>>,
    wait_thumbs_until: Option<Instant>,
    /// 연속 표면 재구성 횟수 — 계속되면 렌더러를 다시 만든다
    reconfigures: u32,
}

impl App {
    fn new(args: Args, registry: DecoderRegistry, proxy: EventLoopProxy<UserEvent>) -> Self {
        let registry = Arc::new(registry);
        let extensions = registry.known_extensions();
        // 형식 묶음 (필터 단위 = 스캔 비트). 지금은 사진만 — 비디오·문서 모드는 묶음을 덧붙인다.
        let groups = iv_core::formats::media_groups(&extensions);
        let loader_cfg = LoaderConfig::default();
        let gate = loader_cfg.gate.clone();
        let p1 = proxy.clone();
        let p2 = proxy.clone();
        let loader = Loader::new(registry.clone(), loader_cfg, move || {
            let _ = p1.send_event(UserEvent::LoaderWake);
        });
        //#region 백그라운드 스캔 — 트리 루트 전부, 현재 장 디코드에 양보
        let tree = FolderTree::with_roots(FolderTree::default_roots());
        let cache_dir = iv_thumbs::default_cache_dir();
        let cache_file = cache_dir.as_ref().map(|d| d.join("scan-v1.tsv"));
        let ignore = cache_dir.into_iter().collect();
        let scanner = Scanner::start(ScanConfig { roots: tree.root_paths(), groups: groups.iter().map(|g| g.exts.clone()).collect(), gate: Some(gate.clone()), ignore, cache_file }, move || {
            let _ = p2.send_event(UserEvent::ScanWake);
        });
        //#endregion
        let codec_proxy = proxy.clone();
        let thumbs = ThumbService::new(registry, ThumbConfig { gate: Some(gate), ..Default::default() }, move || {
            let _ = proxy.send_event(UserEvent::ThumbWake);
        });
        // 변환본 캐시 한도 확인 (시작할 때 한 번, 백그라운드)
        prune_video_cache(None);
        let mut viewer = ViewerState::new([1280.0, 800.0], extensions, ViewerConfig::default());
        viewer.set_kinds(MediaKind::ALL.iter().map(|&k| (k, iv_core::formats::kind_extensions(&groups, k))).collect());
        // 인자 없으면 시작 폴더(IV_START_DIR → 홈)를 브라우저로
        let pending_open = args.path.clone().or_else(|| std::env::var_os("IV_START_DIR").or_else(|| std::env::var_os("HOME")).map(PathBuf::from));
        Self {
            args,
            loader,
            thumbs,
            viewer,
            tree,
            scanner,
            wait_scan_until: None,
            dir_dirty: false,
            tree_rows: Vec::new(),
            tree_counts: Vec::new(),
            tree_rows_at: None,
            tree_dirty: true,
            keep_visible: HashSet::new(),
            cut_paths: HashSet::new(),
            groups,
            cut_count: 0,
            tree_scan_changed: false,
            browser: BrowserState::default(),
            thumb_failed: HashSet::new(),
            last_requested: Vec::new(),
            textures_dir: None,
            preview_area: None,
            gfx: None,
            mods: ModifiersState::empty(),
            cursor: PhysicalPosition::new(0.0, 0.0),
            dragging: None,
            last_click: None,
            repaint_at: None,
            info_rows: Vec::new(),
            pending_open,
            selftest: SelfTest::from_env(),
            video: None,
            video_poll_until: None,
            proxy: codec_proxy,
            codec_installer: None,
            codec_prompt: false,
            codec_declined: false,
            convert: None,
            needs_convert: HashSet::new(),
            pdf: None,
            text_doc: None,
            md_rendered: true,
            injected: VecDeque::new(),
            wait_thumbs_until: None,
            reconfigures: 0,
        }
    }

    fn apply(&mut self, fx: Effects, el: &ActiveEventLoop) {
        // 전체 보기 중 바뀐 열린 폴더는 브라우저로 돌아왔을 때 확인
        if self.dir_dirty && self.viewer.mode == Mode::Browse {
            self.dir_dirty = false;
            let fx2 = self.viewer.refresh_if_changed(&self.loader);
            self.apply(fx2, el);
        }
        // 전체 보기 중엔 썸네일 생성을 멈춘다 (멱등 — 시작 모드·모든 전환을 한 곳에서)
        self.thumbs.set_paused(self.viewer.mode == Mode::View);
        if fx.library_changed {
            let dir = self.viewer.library.dir().map(|d| d.to_path_buf());
            if dir != self.textures_dir {
                self.browser.textures.clear();
                self.thumb_failed.clear();
                // 실패 기록은 폴더 단위로 잊는다 (누적 방지 · 돌아오면 한 번 더 시도)
                self.thumbs.forget_failures();
                self.textures_dir = dir.clone();
            }
            self.last_requested.clear();
            if let Some(d) = &dir {
                self.tree.expand_to(d);
            }
            self.tree_dirty = true;
        }
        if fx.reveal && !self.viewer.library.is_empty() {
            self.browser.scroll_to = Some(self.viewer.library.index());
        }
        if fx.window_changed {
            self.loader.set_window(&self.viewer.window());
        }
        if let Some(req) = fx.app {
            match req {
                AppRequest::Quit => el.exit(),
                AppRequest::ToggleFullscreen => {
                    if let Some(g) = &self.gfx {
                        let fs = g.window.fullscreen().is_some();
                        g.window.set_fullscreen(if fs { None } else { Some(Fullscreen::Borderless(None)) });
                    }
                }
                AppRequest::Trash => self.trash(el),
                AppRequest::FileOp => self.file_op(el),
                AppRequest::Clipboard(op) => self.clipboard(op, el),
                AppRequest::Video(cmd) => self.video_cmd(cmd),
                AppRequest::Page(cmd) => self.page_cmd(cmd, el),
            }
        }
        self.sync_video();
        self.sync_pdf();
        self.sync_text();
        if fx.redraw || fx.window_changed || fx.library_changed || fx.reveal {
            self.update_title();
            if let Some(g) = &self.gfx {
                g.window.request_redraw();
            }
        }
    }

    //#region 비디오 재생 — 전체 보기·비디오 모드의 현재 항목만 재생기를 가진다
    /// 화면 상태에 재생기를 맞춘다 (멱등 — 이동·모드 전환·목록 복귀를 한 곳에서). 바뀌면 옛 것은 멈추고 버린다.
    /// 기본 엔진이 못 여는 영상은 변환본(캐시)을 열고, 없으면 변환을 시작하고, 엔진이 없으면 설치를 권한다.
    fn sync_video(&mut self) {
        let want = (self.viewer.mode == Mode::View && self.viewer.kind() == MediaKind::Video)
            .then(|| self.viewer.current().and_then(|e| e.path().map(|p| (e.id.0, p.to_path_buf()))))
            .flatten();
        let want_key = want.as_ref().map(|(k, _)| *k);
        if self.video.as_ref().map(|(k, _)| *k) == want_key && want_key.is_some() {
            return;
        }
        self.video = None;
        // 다른 영상으로 가면 변환은 멈춘다 (같은 영상이면 이어서)
        if self.convert.as_ref().map(|(k, _)| *k) != want_key {
            self.convert = None;
        }
        let Some((key, path)) = want else { return };
        //#region 1) 기본 엔진
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if iv_decode_video::EXTENSIONS.contains(&ext.as_str()) && !self.needs_convert.contains(&key) {
            match self.open_player(key, &path) {
                Ok(()) => return,
                Err(e) => {
                    log::info!("기본 엔진으로 못 엶 ({e}) → 추가 코덱 엔진: {}", path.display());
                    self.needs_convert.insert(key);
                }
            }
        }
        //#endregion
        //#region 2) 변환본 캐시 → 3) 변환 시작 → 4) 엔진 설치
        let Some(cache) = iv_thumbs::default_cache_dir().map(|d| iv_ffmpeg::cache_path(&d.join("video-v1"), &path)) else { return };
        if cache.is_file() {
            iv_ffmpeg::touch(&cache);
            if let Err(e) = self.open_player(key, &cache) {
                self.viewer.error = Some(format!("재생할 수 없음: {e}"));
            }
            return;
        }
        if let Some((_, job)) = &self.convert {
            match job.state() {
                iv_ffmpeg::JobState::Done(p) => {
                    self.convert = None;
                    prune_video_cache(Some(p.clone()));
                    if let Err(e) = self.open_player(key, &p) {
                        self.viewer.error = Some(format!("재생할 수 없음: {e}"));
                    }
                }
                iv_ffmpeg::JobState::Failed(e) => {
                    self.convert = None;
                    self.viewer.notice = None;
                    self.viewer.error = Some(e);
                }
                iv_ffmpeg::JobState::Running(p, remux) => {
                    self.viewer.notice = Some(if remux { format!("재생 준비 중 (옮겨 담기) {:.0}%", p * 100.0) } else { format!("재생 준비 중 (변환 — 처음 한 번만) {:.0}%", p * 100.0) });
                }
            }
            self.request_redraw();
            return;
        }
        match iv_codecpack::find() {
            Some(t) => {
                log::info!("변환 시작 ({}): {}", t.source, path.display());
                let proxy = self.proxy.clone();
                self.convert = Some((key, iv_ffmpeg::Job::start(t, path, cache, move || {
                    let _ = proxy.send_event(UserEvent::CodecWake);
                })));
                self.viewer.notice = Some("재생 준비 중…".into());
            }
            None => self.need_codec_engine(),
        }
        self.request_redraw();
        //#endregion
    }

    fn open_player(&mut self, key: u64, path: &Path) -> Result<(), String> {
        // 창(물리 픽셀) 긴 변까지만 받아 복사 비용을 줄인다 (4K 영상도 창이 작으면 작게)
        let side = self.gfx.as_ref().map_or(1920, |g| {
            let s = g.window.inner_size();
            s.width.max(s.height)
        });
        // 자가 시험은 소리를 내지 않는다 (사용자 스피커)
        let p = iv_video::VideoPlayer::open(path, side.clamp(640, 3840), self.selftest.is_some())?;
        p.play();
        log::info!("재생: {}", path.display());
        self.video = Some((key, p));
        self.video_poll_until = Some(Instant::now() + Duration::from_secs(2));
        self.viewer.notice = None;
        self.request_redraw();
        Ok(())
    }

    /// 엔진이 필요하다: 동의했으면 바로 설치, 아니면 묻는다 (이번 실행에서 거절했으면 안내만)
    fn need_codec_engine(&mut self) {
        if self.codec_installer.is_some() {
            return;
        }
        if iv_codecpack::consented() {
            self.start_codec_install();
        } else if !self.codec_declined {
            self.codec_prompt = true;
        } else {
            self.viewer.notice = Some("이 영상은 추가 코덱 엔진이 필요합니다 — 보기 메뉴에서 받을 수 있습니다".into());
        }
    }

    fn start_codec_install(&mut self) {
        if self.codec_installer.is_some() || std::env::var_os("IV_CODEC_NO_INSTALL").is_some() {
            return;
        }
        log::info!("코덱 엔진 설치 시작");
        let proxy = self.proxy.clone();
        self.codec_installer = Some(iv_codecpack::Installer::start(move || {
            let _ = proxy.send_event(UserEvent::CodecWake);
        }));
    }

    /// 설치·변환 진행 반영
    fn on_codec_wake(&mut self, el: &ActiveEventLoop) {
        if let Some(inst) = &self.codec_installer {
            match inst.status() {
                iv_codecpack::InstallStatus::Running(msg) => self.viewer.notice = Some(msg),
                iv_codecpack::InstallStatus::Done(t) => {
                    log::info!("코덱 엔진 준비됨: {} ({})", t.ffmpeg.display(), t.source);
                    self.codec_installer = None;
                    self.viewer.notice = Some(format!("코덱 엔진 준비됨 ({})", t.source));
                    // 엔진이 없어 실패했던 썸네일을 다시
                    self.thumbs.forget_failures();
                    self.thumb_failed.clear();
                    self.last_requested.clear();
                }
                iv_codecpack::InstallStatus::Failed(e) => {
                    log::warn!("코덱 엔진 설치 실패: {e}");
                    self.codec_installer = None;
                    self.viewer.notice = None;
                    self.viewer.error = Some(format!("코덱 엔진 설치 실패: {e}"));
                }
            }
        }
        self.sync_video();
        let _ = el;
        self.request_redraw();
    }

    fn video_cmd(&mut self, cmd: VideoCmd) {
        let Some((_, p)) = &self.video else { return };
        match cmd {
            VideoCmd::Toggle => p.toggle(),
            VideoCmd::Seek(t) => p.seek(t),
            VideoCmd::SeekBy(d) => p.seek(p.position() + d),
            VideoCmd::Volume(v) => {
                p.set_volume(v);
                p.set_muted(v <= 0.0);
            }
            VideoCmd::VolumeBy(d) => {
                p.set_volume(p.volume() + d);
                p.set_muted(p.volume() <= 0.0);
            }
            VideoCmd::ToggleMute => p.set_muted(!p.muted()),
        }
        self.video_poll_until = Some(Instant::now() + Duration::from_millis(600));
        self.request_redraw();
    }

    /// 새 프레임이 있으면 화면에. 돌려주는 값 = 다음에 확인할 시각 (재생 중이면 곧, 멈췄으면 None)
    fn pump_video(&mut self) -> Option<Instant> {
        // 열렸지만 디코드를 못 하는 영상(예: MP4 안의 VP9) → 추가 코덱 엔진 경로로
        if let Some((key, _)) = self.video.as_ref().filter(|(_, p)| p.failed()) {
            let key = *key;
            log::info!("기본 엔진 재생 실패 → 추가 코덱 엔진");
            self.needs_convert.insert(key);
            self.video = None;
            self.sync_video();
        }
        let (key, p) = self.video.as_mut()?;
        let key = *key;
        if let Some(f) = p.poll_frame() {
            let fx = self.viewer.show_live(key, Arc::new(f));
            if fx.redraw {
                self.request_redraw();
            }
        }
        let now = Instant::now();
        let playing = self.video.as_ref().is_some_and(|(_, p)| p.is_playing());
        if playing {
            // 시간 표시도 따라가게 — 새 프레임이 없어도 가끔 다시 그린다
            Some(now + Duration::from_millis(8))
        } else {
            self.video_poll_until.filter(|t| *t > now).map(|_| now + Duration::from_millis(16))
        }
    }
    //#endregion

    //#region PDF 전체 보기 — 쪽 넘기기·확대 다시 그리기 (그리기는 백그라운드)
    /// 화면 상태에 PDF 보기를 맞춘다 (멱등)
    fn sync_pdf(&mut self) {
        let want = (self.viewer.mode == Mode::View && self.viewer.kind() == MediaKind::Document)
            .then(|| self.viewer.current().and_then(|e| e.path().filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf"))).map(|p| (e.id.0, p.to_path_buf()))))
            .flatten();
        if self.pdf.as_ref().map(|p| p.key) == want.as_ref().map(|(k, _)| *k) {
            return;
        }
        self.pdf = want.map(|(key, path)| {
            let pages = iv_decode_pdf::page_count(&path).unwrap_or(1).max(1);
            PdfView { key, path, pages, page: 0, rendered: 0, job: None }
        });
        if self.pdf.is_some() {
            self.render_pdf_page(self.pdf_side(None));
        }
    }

    /// 화면 상태에 글 문서 보기를 맞춘다 (멱등 — 2MB 까지 바로 읽는다)
    fn sync_text(&mut self) {
        let want = (self.viewer.mode == Mode::View && self.viewer.kind() == MediaKind::Document)
            .then(|| self.viewer.current().and_then(|e| e.path().filter(|p| crate::textdoc::text_kind(p).is_some()).map(|p| (e.id.0, p.to_path_buf()))))
            .flatten();
        if self.text_doc.as_ref().map(|(k, _)| *k) == want.as_ref().map(|(k, _)| *k) {
            return;
        }
        self.text_doc = None;
        if let Some((key, path)) = want {
            match crate::textdoc::load(&path) {
                Ok(t) => self.text_doc = Some((key, t)),
                Err(e) => self.viewer.error = Some(e),
            }
        }
        self.request_redraw();
    }

    /// 그릴 긴 변: 화면이 필요로 하는 만큼 (창 긴 변 이상, 8192 이하)
    fn pdf_side(&self, need: Option<u32>) -> u32 {
        let win = self.gfx.as_ref().map_or(1600, |g| {
            let s = g.window.inner_size();
            s.width.max(s.height)
        });
        need.unwrap_or(win).max(win).clamp(512, 8192)
    }

    fn render_pdf_page(&mut self, side: u32) {
        let Some(pv) = &mut self.pdf else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        let (path, page) = (pv.path.clone(), pv.page);
        let proxy = self.proxy.clone();
        let _ = std::thread::Builder::new().name("iv-pdf-page".into()).spawn(move || {
            let _ = tx.send(iv_decode_pdf::render_page(&path, page, side));
            let _ = proxy.send_event(UserEvent::DocWake);
        });
        pv.job = Some((page, side, rx));
    }

    /// 그린 쪽 받기 + 확대로 더 큰 해상도가 필요하면 다시 그리기
    fn pump_pdf(&mut self) {
        let Some(pv) = &mut self.pdf else { return };
        if let Some((page, side, rx)) = &pv.job {
            match rx.try_recv() {
                Ok(r) => {
                    let (page, side) = (*page, *side);
                    pv.job = None;
                    // 그리는 동안 쪽을 넘겼으면 버린다
                    if page == pv.page {
                        match r {
                            Ok(f) => {
                                let new_page = pv.rendered == 0;
                                pv.rendered = side;
                                let key = pv.key;
                                let fx = self.viewer.show_page(key, Arc::new(f), new_page);
                                if fx.redraw {
                                    self.request_redraw();
                                }
                            }
                            Err(e) => self.viewer.error = Some(e),
                        }
                    } else {
                        let s = self.pdf_side(None);
                        self.render_pdf_page(s);
                    }
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => pv.job = None,
            }
        }
        let Some(pv) = &self.pdf else { return };
        let need = self.viewer.viewport.needed_max_side();
        if pv.rendered > 0 && need as f32 > pv.rendered as f32 * 1.2 && pv.rendered < 8192 {
            let s = self.pdf_side(Some(need));
            self.render_pdf_page(s);
        }
    }

    fn page_cmd(&mut self, cmd: PageCmd, el: &ActiveEventLoop) {
        let Some(pv) = &mut self.pdf else {
            // PDF 가 아닌 문서: 쪽 명령은 파일 넘기기로
            let a = match cmd {
                PageCmd::Next => Action::Next,
                PageCmd::Prev => Action::Prev,
                PageCmd::First => Action::First,
                PageCmd::Last => Action::Last,
                PageCmd::Go(_) => return,
            };
            let fx = self.viewer.handle(a, &self.loader);
            self.apply(fx, el);
            return;
        };
        let last = pv.pages - 1;
        let to = match cmd {
            PageCmd::Next => (pv.page + 1).min(last),
            PageCmd::Prev => pv.page.saturating_sub(1),
            PageCmd::First => 0,
            PageCmd::Last => last,
            PageCmd::Go(n) => n.min(last),
        };
        if to == pv.page {
            // 끝 쪽에서 다음 → 다음 파일, 첫 쪽에서 이전 → 이전 파일 (전체 보기는 순환)
            let a = match cmd {
                PageCmd::Next => Action::Next,
                PageCmd::Prev => Action::Prev,
                _ => return,
            };
            let fx = self.viewer.handle(a, &self.loader);
            self.apply(fx, el);
            return;
        }
        pv.page = to;
        pv.rendered = 0;
        let s = self.pdf_side(None);
        self.render_pdf_page(s);
        self.update_title();
    }
    //#endregion

    //#region 휴지통 — 뷰어가 고른 파일을 OS 휴지통으로, 성공한 것만 목록에서 뺀다
    fn trash(&mut self, el: &ActiveEventLoop) {
        let mut targets = self.viewer.trash_targets();
        // 자가 시험은 시험 루트(IV_TREE_ROOTS) 안의 파일만 지울 수 있다 — 사용자 파일 보호
        if self.selftest.is_some() {
            let roots: Vec<PathBuf> = std::env::var_os("IV_TREE_ROOTS")
                .map(|v| std::env::split_paths(&v).filter_map(|p| p.canonicalize().ok()).collect())
                .unwrap_or_default();
            targets.retain(|p| {
                let ok = std::env::var_os("IV_TRASH_DIR").is_some() && roots.iter().any(|r| p.starts_with(r));
                if !ok {
                    log::warn!("자가 시험: 시험 루트 밖 파일은 지우지 않음 {}", p.display());
                }
                ok
            });
        }
        let (mut moved, mut failures) = (Vec::new(), Vec::new());
        for p in targets {
            match iv_trash::trash(&p) {
                Ok(()) => moved.push(p),
                Err(e) => {
                    log::warn!("휴지통 이동 실패 {}: {e}", p.display());
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    failures.push(format!("{name} ({e})"));
                }
            }
        }
        log::info!("휴지통: {}개 이동, {}개 실패", moved.len(), failures.len());
        let fx = self.viewer.on_trashed(&moved, &failures, &self.loader);
        self.apply(fx, el);
    }
    //#endregion

    fn request_redraw(&self) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    /// 자가 시험 중엔 시험 루트(IV_TREE_ROOTS) 안만 건드린다 — 사용자 파일 보호
    fn selftest_allows(&self, p: &Path) -> bool {
        if self.selftest.is_none() {
            return true;
        }
        let roots: Vec<PathBuf> = std::env::var_os("IV_TREE_ROOTS").map(|v| std::env::split_paths(&v).filter_map(|p| p.canonicalize().ok()).collect()).unwrap_or_default();
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let ok = roots.iter().any(|r| p.starts_with(r) && &p != r);
        if !ok {
            log::warn!("자가 시험: 시험 루트 밖은 건드리지 않음 {}", p.display());
        }
        ok
    }

    //#region 파일·폴더 작업 — 디스크 작업은 여기(iv-fsops·iv-rotate), 목록 반영은 뷰어, 트리 반영은 여기
    fn file_op(&mut self, el: &ActiveEventLoop) {
        let Some(op) = self.viewer.take_file_op() else { return };
        match &op {
            FileOp::MoveTo { dest } => return self.move_to(dest.clone(), el),
            FileOp::RotateMarked { quarters } => return self.rotate_marked(*quarters, el),
            FileOp::Paste { dest } => return self.paste(dest.clone(), el),
            _ => {}
        }
        let target = match &op {
            FileOp::NewFolder { parent, .. } => parent.join("x"),
            FileOp::RenameFolder { path, .. } | FileOp::RenameFile { path, .. } | FileOp::Rotate { path, .. } => path.clone(),
            FileOp::MoveTo { dest } | FileOp::Paste { dest } => dest.clone(),
            FileOp::RotateMarked { .. } => PathBuf::new(),
        };
        let result = if !self.selftest_allows(&target) {
            Err("시험 루트 밖".to_string())
        } else {
            match &op {
                FileOp::NewFolder { parent, name } => iv_fsops::create_folder(parent, name),
                FileOp::RenameFolder { path, name } | FileOp::RenameFile { path, name } => iv_fsops::rename(path, name),
                FileOp::Rotate { path, quarters } => rotate_file(path, *quarters),
                FileOp::MoveTo { .. } | FileOp::RotateMarked { .. } | FileOp::Paste { .. } => Err("내부 오류".to_string()),
            }
        };
        match (&op, &result) {
            (FileOp::NewFolder { parent, .. }, Ok(p)) => {
                log::info!("새 폴더: {}", p.display());
                // 새 폴더는 비어 있어도 트리에 보이게 (스캔이 '사진 없음'으로 숨기지 않게)
                self.keep_visible.insert(p.clone());
                self.tree.invalidate(parent);
                self.tree.expand_to(p);
            }
            (FileOp::RenameFolder { path, .. }, Ok(p)) => {
                log::info!("폴더 이름 바꿈: {} → {}", path.display(), p.display());
                self.keep_visible.remove(path);
                self.keep_visible.insert(p.clone());
                self.tree.renamed(path, p);
            }
            (FileOp::RenameFile { path, .. }, Ok(p)) => log::info!("이름 바꿈: {} → {}", path.display(), p.display()),
            (FileOp::Rotate { quarters, .. }, Ok(p)) => log::info!("돌려 저장 ({}°): {}", *quarters as u32 * 90, p.display()),
            (_, Err(e)) => log::warn!("파일 작업 실패: {e}"),
            (FileOp::MoveTo { .. } | FileOp::RotateMarked { .. } | FileOp::Paste { .. }, Ok(_)) => {}
        }
        self.tree_dirty = true;
        let fx = self.viewer.on_file_op(&op, &result, &self.loader);
        self.apply(fx, el);
    }
    //#region 일괄 회전·클립보드
    /// 선택한 사진들을 돌려 저장 (방향 태그만)
    fn rotate_marked(&mut self, quarters: u8, el: &ActiveEventLoop) {
        let (mut done, mut failures) = (Vec::new(), Vec::new());
        for p in self.viewer.trash_targets() {
            if !self.selftest_allows(&p) {
                continue;
            }
            match rotate_file(&p, quarters) {
                Ok(_) => done.push(p),
                Err(e) => failures.push(format!("{} ({e})", file_name(&p))),
            }
        }
        log::info!("일괄 회전 {}°: {}장, 실패 {}", quarters as u32 * 90, done.len(), failures.len());
        let fx = self.viewer.on_rotated(&done, &failures, &self.loader);
        self.apply(fx, el);
    }

    /// 자가 시험 중엔 시험용 클립보드 파일만 (사용자 클립보드를 덮지 않게)
    fn clip_allowed(&self) -> Result<(), String> {
        clip_ok(self.selftest.is_some())
    }

    fn write_clip_text(&self, t: &str) -> Result<i64, String> {
        self.clip_allowed()?;
        iv_clipboard::write_text(t)
    }

    fn clipboard(&mut self, op: ClipOp, el: &ActiveEventLoop) {
        let result = match op {
            ClipOp::Copy | ClipOp::Cut => {
                let files = self.viewer.trash_targets();
                let r = self.clip_allowed().and_then(|()| iv_clipboard::write_files(&files));
                self.cut_paths.clear();
                r.map(|count| {
                    if op == ClipOp::Cut {
                        self.cut_paths = files.iter().cloned().collect();
                        self.cut_count = count;
                        format!("{}장 잘라냄 — 옮길 폴더에서 ⌘V", files.len())
                    } else {
                        format!("{}장 복사함 — Finder 에도 붙여넣을 수 있음", files.len())
                    }
                })
            }
            ClipOp::CopyPath => {
                let paths = self.viewer.path_targets();
                let text = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
                self.write_clip_text(&text).map(|_| if paths.len() > 1 { format!("경로 {}개 복사함", paths.len()) } else { format!("경로 복사함: {text}") })
            }
        };
        if let Err(e) = &result {
            log::warn!("클립보드: {e}");
        }
        let fx = self.viewer.on_clipboard(result);
        self.apply(fx, el);
    }

    /// 클립보드의 파일을 `dest` 에 — 이 앱에서 잘라낸 그대로면 옮기고, 아니면 복사 (덮어쓰지 않고 "사본" 이름)
    fn paste(&mut self, dest: PathBuf, el: &ActiveEventLoop) {
        let files = if self.clip_allowed().is_ok() { iv_clipboard::read_files() } else { Vec::new() };
        let is_cut = !self.cut_paths.is_empty()
            && iv_clipboard::change_count() == self.cut_count
            && files.len() == self.cut_paths.len()
            && files.iter().all(|f| self.cut_paths.contains(f));
        let (mut created, mut moved_from, mut failures) = (Vec::new(), Vec::new(), Vec::new());
        if files.is_empty() {
            failures.push("클립보드에 파일이 없음".to_string());
        }
        if self.selftest_allows(&dest) {
            for f in &files {
                if !self.selftest_allows(f) {
                    continue;
                }
                if is_cut && f.parent() == Some(dest.as_path()) {
                    continue; // 같은 폴더로 옮기기 = 할 일 없음
                }
                let r = if is_cut { iv_fsops::move_into(f, &dest) } else { iv_fsops::copy_into(f, &dest) };
                match r {
                    Ok(to) => {
                        if is_cut {
                            moved_from.push(f.clone());
                        }
                        created.push(to);
                    }
                    Err(e) => failures.push(format!("{} ({e})", file_name(f))),
                }
            }
        }
        if is_cut {
            self.cut_paths.clear();
            // 옮긴 뒤엔 클립보드가 새 위치를 가리키게 (다시 ⌘V 하면 복사)
            if !created.is_empty() && self.clip_allowed().is_ok() {
                let _ = iv_clipboard::write_files(&created);
            }
        }
        log::info!("붙여넣기 → {}: {}장 ({}), 실패 {}", dest.display(), created.len(), if is_cut { "옮김" } else { "복사" }, failures.len());
        // 트리 숫자: 대상과 (옮겼으면) 원래 폴더
        self.tree.invalidate(&dest);
        for f in &moved_from {
            if let Some(d) = f.parent() {
                self.tree.invalidate(d);
            }
        }
        self.tree_dirty = true;
        let fx = self.viewer.on_pasted(&dest, &created, &moved_from, &failures, &self.loader);
        self.apply(fx, el);
    }
    //#endregion

    /// 선택한 사진들을 폴더로 옮긴다 (같은 이름이 있으면 그 사진은 남기고 오류로 알린다)
    fn move_to(&mut self, dest: PathBuf, el: &ActiveEventLoop) {
        let targets = self.viewer.trash_targets();
        let (mut moved, mut failures) = (Vec::new(), Vec::new());
        if self.selftest_allows(&dest) {
            for p in targets {
                if !self.selftest_allows(&p) {
                    continue;
                }
                match iv_fsops::move_into(&p, &dest) {
                    Ok(_) => moved.push(p),
                    Err(e) => {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        failures.push(format!("{name} ({e})"));
                    }
                }
            }
        }
        log::info!("옮기기 → {}: {}개 성공, {}개 실패", dest.display(), moved.len(), failures.len());
        self.tree.invalidate(&dest);
        self.tree_dirty = true;
        let fx = self.viewer.on_moved(&moved, &failures, &self.loader);
        self.apply(fx, el);
    }
    //#endregion

    /// 창이 있는 모니터의 긴 변 (물리 픽셀)
    fn monitor_side(&self) -> Option<u32> {
        let m = self.gfx.as_ref()?.window.current_monitor()?;
        let s = m.size();
        Some(s.width.max(s.height))
    }

    fn handle(&mut self, a: Action, el: &ActiveEventLoop) {
        // 필터가 바뀌면 트리(숨김·숫자)도 그 필터로 다시 만든다 — 다시 훑지 않고 조회만
        if matches!(a, Action::SetFilter(_) | Action::SetKind(_)) {
            self.tree_dirty = true;
        }
        //#region 코덱 엔진 받기 창의 답
        match &a {
            Action::InstallCodecs => {
                self.codec_prompt = false;
                iv_codecpack::set_consent();
                self.start_codec_install();
                self.request_redraw();
                return;
            }
            Action::ToggleMarkdown => {
                self.md_rendered = !self.md_rendered;
                self.request_redraw();
                return;
            }
            Action::DismissCodecPrompt => {
                self.codec_prompt = false;
                self.codec_declined = true;
                self.request_redraw();
                return;
            }
            _ => {}
        }
        //#endregion
        //#region 이름 입력 대화 상자 열기 — UI 상태라 앱이 연다 (브라우저에서만)
        match &a {
            Action::BeginRename if self.viewer.mode == Mode::Browse => {
                if let Some(p) = self.viewer.current().and_then(|e| e.path()).map(Path::to_path_buf) {
                    self.browser.begin_rename_file(&p);
                    self.request_redraw();
                }
                return;
            }
            Action::BeginNewFolder if self.viewer.mode == Mode::Browse => {
                if let Some(d) = self.viewer.library.dir().map(Path::to_path_buf) {
                    self.browser.begin_new_folder(&d);
                    self.request_redraw();
                }
                return;
            }
            _ => {}
        }
        //#endregion
        if a == Action::Refresh {
            // 새로고침 = 고쳐졌을 수 있는 파일의 썸네일을 다시 시도
            self.thumbs.forget_failures();
            self.thumb_failed.clear();
        }
        let fx = self.viewer.handle(a, &self.loader);
        self.apply(fx, el);
    }

    fn renderer_name(&self) -> String {
        self.gfx.as_ref().map(|g| g.renderer.name().to_string()).unwrap_or_default()
    }

    fn status(&self) -> String {
        let r = self.renderer_name();
        match self.viewer.mode {
            // 글 문서는 이미지 정보(크기·배율·디코드 시간)가 의미 없다 — 이름과 위치만
            Mode::View if self.text_doc.is_some() => {
                let lib = &self.viewer.library;
                let name = lib.current().map(|e| e.name.as_str()).unwrap_or("");
                format!("{name}  ·  {}/{}", lib.index() + 1, lib.len())
            }
            Mode::View => self.viewer.status_line(&r),
            Mode::Browse => {
                let size = self.viewer.current().and_then(|e| self.browser.textures.full_size(e.id.0));
                self.viewer.browse_status(size, &r)
            }
        }
    }

    fn update_title(&mut self) {
        let Some(g) = &self.gfx else { return };
        let title = match (self.viewer.mode, self.viewer.current()) {
            (Mode::View, Some(e)) => format!("{} — {APP}", e.name),
            (Mode::Browse, _) => match self.viewer.library.dir() {
                Some(d) => format!("{} — {APP}", iv_fstree::display_name(d)),
                None => APP.to_string(),
            },
            _ => APP.to_string(),
        };
        // UI 를 못 그리는 렌더러(CPU)는 상태 전체를 제목에 싣는다
        let title = if g.renderer.ui_capability() == UiCapability::StatusOnly { self.status() } else { title };
        // 자가 시험 창은 실제 입력을 무시한다 — 사용자가 진짜 창으로 착각하지 않게 제목에 밝힌다
        let title = if self.selftest.is_some() { format!("[자가 시험 — 입력 무시] {title}") } else { title };
        g.window.set_title(&title);
    }

    fn refresh_info(&mut self) {
        self.info_rows.clear();
        let Some(e) = self.viewer.current() else { return };
        self.info_rows.push(("파일".into(), e.name.clone()));
        if let Some(dir) = self.viewer.library.dir() {
            self.info_rows.push(("폴더".into(), dir.display().to_string()));
        }
        if let Some(s) = &self.viewer.shown {
            let [w, h] = s.frame.display_full_size();
            self.info_rows.push(("원본".into(), format!("{w} × {h}")));
            self.info_rows.push(("디코드".into(), format!("{} × {}{}", s.frame.width, s.frame.height, if s.frame.is_preview { " (미리보기)" } else { "" })));
            self.info_rows.push(("방향".into(), format!("{:?}", s.frame.orientation)));
        }
        let (cache, queued, running) = self.loader.stats();
        self.info_rows.push(("캐시".into(), format!("{} MB / {} MB · {}장", cache.bytes >> 20, cache.limit >> 20, cache.frames)));
        self.info_rows.push(("작업".into(), format!("대기 {queued} · 진행 {running}")));
        let (tb, tn, tq, tr) = self.thumbs.stats();
        self.info_rows.push(("썸네일".into(), format!("{} MB · {}장 · 대기 {tq} · 진행 {tr}", tb >> 20, tn)));
        if let Some(g) = &self.gfx {
            self.info_rows.push(("렌더러".into(), g.renderer.name().to_string()));
        }
        self.info_rows.push(("버전".into(), APP.to_string()));
    }

    /// 한 프레임: UI → 장면 → 렌더 (또는 캡처). redraw 와 자가 시험 캡처가 같은 길을 쓴다.
    fn frame(&mut self, el: &ActiveEventLoop, capture: Option<PathBuf>) {
        if self.viewer.show_info {
            self.refresh_info();
        }
        let status = self.status();
        let (ui, ui_actions) = self.build_ui(&status);
        let Some(g) = &mut self.gfx else { return };
        let cur = self.viewer.current().map(|e| e.id.0);
        let browse = self.viewer.mode == Mode::Browse;
        let image = self.viewer.shown.as_ref().filter(|s| !browse || Some(s.key) == cur).map(|s| (s.key, &s.frame));
        let scene = if browse {
            // 미리보기 칸이 아직 없으면 빈 영역 (None 이면 화면 전체에 그려진다)
            Scene { viewport: &self.viewer.preview, image, image_area: Some(self.preview_area.unwrap_or([0.0; 4])), background: BACKGROUND, ui }
        } else {
            Scene { viewport: &self.viewer.viewport, image, image_area: None, background: BACKGROUND, ui }
        };
        if let Some(path) = capture {
            match g.renderer.capture(scene) {
                Some((w, h, px)) => {
                    let ok = image::save_buffer(&path, &px, w, h, image::ExtendedColorType::Rgba8).is_ok();
                    log::info!("자가 시험 캡처 {} {w}x{h} {} | {status} | {}", path.display(), if ok { "저장" } else { "실패" }, { let k = self.viewer.kind(); self.scanner.status().text_for(kind_mask(&self.groups, &self.viewer), k.label(), k.unit()) });
                }
                None => log::warn!("자가 시험 캡처 실패"),
            }
        } else {
            let outcome = g.renderer.render(scene);
            log::trace!("render → {outcome:?}");
            match outcome {
                RenderOutcome::Ok | RenderOutcome::Skipped => self.reconfigures = 0,
                RenderOutcome::Reconfigure => {
                    self.reconfigures += 1;
                    if self.reconfigures > 3 {
                        // 표면이 계속 사라지면(Lost) 같은 단계 렌더러를 새로 만든다
                        log::warn!("표면 재구성 반복 → 렌더러 재생성");
                        self.reconfigures = 0;
                        let level = g.level;
                        self.switch_renderer(level);
                    } else {
                        let s = g.window.inner_size();
                        g.renderer.resize([s.width, s.height]);
                        g.window.request_redraw();
                    }
                }
                RenderOutcome::DeviceLost(why) => {
                    log::error!("렌더러 장치 손실: {why}");
                    if let Some(next) = renderer::next_level(g.level) {
                        self.switch_renderer(next);
                    }
                }
            }
        }
        for a in ui_actions {
            self.handle(a, el);
        }
    }

    /// egui 한 프레임을 돌려 그리기 목록과 버튼 Action 을 만든다
    fn build_ui(&mut self, status: &str) -> (Option<EguiPaint>, Vec<Action>) {
        let mut ui_actions = Vec::new();
        let mut prompt_actions: Vec<Action> = Vec::new();
        let Some(g) = &mut self.gfx else { return (None, ui_actions) };
        let Some((ctx, state)) = &mut g.egui else { return (None, ui_actions) };
        let mut raw = state.take_egui_input(&g.window);
        if let Some(batch) = self.injected.pop_front() {
            raw.events.extend(batch);
            g.window.request_redraw();
        }
        let browse = self.viewer.mode == Mode::Browse;
        let mut bout = None;
        let out = if browse {
            let lib = &self.viewer.library;
            //#region 트리: 스캔으로 '사진 없음'이 확인된 폴더는 숨긴다 (열려 있는 폴더와 그 조상은 예외)
            let pointer_down = ctx.input(|i| i.pointer.any_down());
            let stale = self.tree_rows_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
            if self.tree_dirty || (self.tree_scan_changed && stale && !pointer_down) {
                self.tree_scan_changed = false;
                let open_dir = lib.dir();
                let scanner = &self.scanner;
                let keep = &self.keep_visible;
                let show_all = self.browser.show_empty_folders;
                // 필터 = 고른 확장자가 든 묶음들, 필터 없음 = 사진 묶음 전부 (모드의 전체 — 나중 비디오 묶음은 빠진다)
                let mask = tree_mask(&self.groups, &self.viewer);
                let hide = |p: &Path| !show_all && open_dir.is_none_or(|d| !d.starts_with(p)) && !keep.iter().any(|k| k.starts_with(p)) && scanner.verdict_for(p, mask) == Verdict::Empty;
                self.tree_rows = self.tree.visible_rows_where(&hide);
                self.tree_counts = self.tree_rows.iter().map(|r| scanner.counts_for(&r.path, mask)).collect();
                self.tree_rows_at = Some(Instant::now());
                self.tree_dirty = false;
            }
            // 파일 존재만 보는 가벼운 확인 (설치 직후 메뉴가 바로 바뀌게)
            let codec_src = iv_codecpack::find().map(|t| t.source);
            let kind = self.viewer.kind();
            let scan_status = self.scanner.status().text_for(kind_mask(&self.groups, &self.viewer), kind.label(), kind.unit());
            let kind_groups: Vec<iv_core::formats::FormatGroup> = self.groups.iter().filter(|g| g.kind == kind).cloned().collect();
            //#endregion
            let cur = lib.current().map(|e| e.id.0);
            let model = BrowserModel {
                entries: lib.entries(),
                selected: (!lib.is_empty()).then(|| lib.index()),
                marked: self.viewer.marked(),
                dir: lib.dir(),
                tree: &self.tree_rows,
                tree_counts: &self.tree_counts,
                status,
                sort: lib.sort_order(),
                failed: &self.thumb_failed,
                scan_status: &scan_status,
                cut: &self.cut_paths,
                groups: &kind_groups,
                kind,
                codec_engine: codec_src,
                filter: self.viewer.filter(),
                preview_has_frame: self.viewer.shown.as_ref().is_some_and(|s| Some(s.key) == cur),
            };
            let browser = &mut self.browser;
            let prompt = self.codec_prompt;
            ctx.run_pass(raw, |ctx| {
                bout = Some(iv_ui::browser::draw(ctx, browser, &model));
                if prompt {
                    prompt_actions.extend(iv_ui::codec_prompt(ctx));
                }
            })
        } else {
            //#region 글 문서 머리 글 — 이름, 인코딩·잘림 안내, 마크다운인가
            let text_name = self.viewer.current().map(|e| e.name.clone()).unwrap_or_default();
            let text_is_md = text_name.to_ascii_lowercase().ends_with(".md") || text_name.to_ascii_lowercase().ends_with(".markdown");
            let text_note = self.text_doc.as_ref().map_or(String::new(), |(_, t)| {
                let lines = t.text.lines().count();
                format!("{} · {}줄{}", t.encoding, lines, if t.truncated { " · 앞 2MB 만" } else { "" })
            });
            //#endregion
            let model = iv_ui::UiModel {
                status,
                show_info: self.viewer.show_info,
                info: &self.info_rows,
                has_image: self.viewer.current().is_some(),
                can_browse: self.viewer.browse_available,
                rotated: self.viewer.turns() != 0,
                can_rotate: self.viewer.kind() == MediaKind::Image,
                pages: self.pdf.as_ref().map(|p| (p.page, p.pages)),
                text: self.text_doc.as_ref().map(|(_, t)| iv_ui::TextView {
                    name: text_name.as_str(),
                    text: &t.text,
                    code: t.code && !text_is_md,
                    markdown: text_is_md,
                    rendered: self.md_rendered,
                    note: text_note.as_str(),
                }),
                video: self.video.as_ref().map(|(_, p)| iv_ui::VideoUi { playing: p.is_playing(), position: p.position(), duration: p.duration(), volume: p.volume(), muted: p.muted() }),
            };
            let prompt = self.codec_prompt;
            ctx.run_pass(raw, |ctx| {
                ui_actions = iv_ui::draw(ctx, &model);
                if prompt {
                    prompt_actions.extend(iv_ui::codec_prompt(ctx));
                }
            })
        };
        state.handle_platform_output(&g.window, out.platform_output);
        let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
        self.repaint_at = (delay < Duration::from_secs(10)).then(|| Instant::now() + delay);
        // 미뤄 둔 트리 갱신이 있으면 곧 다시 그린다
        if browse && self.tree_scan_changed {
            let t = Instant::now() + Duration::from_millis(300);
            self.repaint_at = Some(self.repaint_at.map_or(t, |r| r.min(t)));
        }
        let ppp = out.pixels_per_point;
        let primitives = ctx.tessellate(out.shapes, ppp);
        let paint = EguiPaint { primitives, textures_delta: out.textures_delta, pixels_per_point: ppp };
        if let Some(b) = bout {
            if b.tree_changed {
                self.tree_dirty = true;
                g.window.request_redraw();
            }
            ui_actions.extend(b.actions);
            if let Some(t) = b.copy_text {
                let r = clip_ok(self.selftest.is_some()).and_then(|()| iv_clipboard::write_text(&t)).map(|_| format!("경로 복사함: {t}"));
                let _ = self.viewer.on_clipboard(r);
            }
            let window = g.window.clone();
            self.after_browser(b.tree_toggle, b.visible, b.preview_rect.map(|r| [r.min.x * ppp, r.min.y * ppp, r.max.x * ppp, r.max.y * ppp]), b.grid_cols, b.grid_rows_visible, &window);
        }
        ui_actions.extend(prompt_actions);
        (Some(paint), ui_actions)
    }

    /// 브라우저 레이아웃 결과 반영: 트리 펼침, 미리보기 칸, 격자 열 수, 썸네일 요청
    fn after_browser(&mut self, toggle: Option<PathBuf>, visible: std::ops::Range<usize>, area: Option<[f32; 4]>, cols: usize, rows: usize, window: &Window) {
        let mut redraw = false;
        if let Some(p) = toggle {
            self.tree.toggle(&p);
            self.tree_dirty = true;
            redraw = true;
        }
        if area != self.preview_area {
            self.preview_area = area;
            if let Some([x0, y0, x1, y1]) = area {
                let _ = self.viewer.set_preview_screen([(x1 - x0).max(1.0), (y1 - y0).max(1.0)]);
            }
            redraw = true;
        }
        self.viewer.grid_cols = cols;
        self.viewer.grid_rows_visible = rows;
        // 보이는 항목 키 목록이 바뀌었을 때만 요청 (스크롤·정렬·열 수·폴더 변경을 한 규칙으로)
        let entries = self.viewer.library.entries();
        let vis = &entries[visible.start.min(entries.len())..visible.end.min(entries.len())];
        let keys: Vec<u64> = vis.iter().map(|e| e.id.0).collect();
        // 메모리 캐시에 있는 것은 바로 텍스처로
        for e in vis {
            let k = e.id.0;
            if !self.browser.textures.has(k) {
                if let Some(t) = self.thumbs.get(k) {
                    self.browser.textures.push(k, t.image, t.full_size);
                    redraw = true;
                } else if !self.thumb_failed.contains(&k) && self.thumbs.failed(k).is_some() {
                    self.thumb_failed.insert(k);
                }
            }
        }
        if keys != self.last_requested {
            let reqs: Vec<ThumbRequest> = vis
                .iter()
                .filter(|e| !self.browser.textures.has(e.id.0) && !self.thumb_failed.contains(&e.id.0))
                .map(|e| ThumbRequest { key: e.id.0, src: e.source.clone(), mtime: e.mtime, len: e.len })
                .collect();
            self.thumbs.request(&reqs);
            self.last_requested = keys;
        }
        if redraw || self.browser.textures.has_pending() {
            window.request_redraw();
        }
    }

    /// 보이는 썸네일이 다 찼는가 (자가 시험 wait-thumbs)
    fn thumbs_complete(&self) -> bool {
        let lib = &self.viewer.library;
        if lib.is_empty() {
            return true;
        }
        !self.last_requested.is_empty()
            && !self.browser.textures.has_pending()
            && self.last_requested.iter().all(|k| self.browser.textures.has(*k) || self.thumb_failed.contains(k))
            && lib.entries().len() >= self.last_requested.len()
    }

    /// 자가 시험 단계 실행
    fn run_selftest(&mut self, el: &ActiveEventLoop) -> Option<Instant> {
        if let Some(until) = self.wait_scan_until {
            if Instant::now() < until && !self.scanner.status().done {
                return Some(Instant::now() + Duration::from_millis(30));
            }
            log::info!("자가 시험: 스캔 {}", if self.scanner.status().done { "완료" } else { "시간 초과" });
            self.wait_scan_until = None;
            if let Some(g) = &self.gfx {
                g.window.request_redraw();
            }
        }
        if let Some(until) = self.wait_thumbs_until {
            if Instant::now() < until && !self.thumbs_complete() {
                return Some(Instant::now() + Duration::from_millis(30));
            }
            log::info!("자가 시험: 썸네일 {}", if self.thumbs_complete() { "모두 채워짐" } else { "시간 초과" });
            self.wait_thumbs_until = None;
        }
        // 주입한 입력이 다 소화될 때까지 다음 단계를 미룬다
        if !self.injected.is_empty() {
            if let Some(g) = &self.gfx {
                g.window.request_redraw();
            }
            return Some(Instant::now() + Duration::from_millis(16));
        }
        loop {
            let (step, wake) = self.selftest.as_mut()?.next();
            let Some(step) = step else { return wake };
            log::info!("자가 시험: {step:?}");
            let none = egui::Modifiers::default();
            match step {
                Step::Act(a) => self.handle(a, el),
                Step::Quit => el.exit(),
                Step::Resize(w, h) => {
                    if let Some(g) = &self.gfx {
                        let _ = g.window.request_inner_size(winit::dpi::PhysicalSize::new(w, h));
                    }
                }
                Step::DeviceLost => {
                    let next = self.gfx.as_ref().and_then(|g| renderer::next_level(g.level));
                    if let Some(n) = next {
                        self.switch_renderer(n);
                    }
                }
                Step::Capture(path) => self.frame(el, Some(path)),
                Step::Click(x, y) | Step::DoubleClick(x, y) | Step::RightClick(x, y) => {
                    let pos = egui::pos2(x, y);
                    let button = if matches!(step, Step::RightClick(..)) { egui::PointerButton::Secondary } else { egui::PointerButton::Primary };
                    let press = |p: bool| egui::Event::PointerButton { pos, button, pressed: p, modifiers: none };
                    self.injected.push_back(vec![egui::Event::PointerMoved(pos)]);
                    self.injected.push_back(vec![press(true), press(false)]);
                    if matches!(step, Step::DoubleClick(..)) {
                        self.injected.push_back(vec![press(true), press(false)]);
                    }
                    self.injected.push_back(vec![]);
                    if let Some(g) = &self.gfx {
                        g.window.request_redraw();
                    }
                    return Some(Instant::now() + Duration::from_millis(16));
                }
                //#region 끌어다 놓기 — 누름 → 여러 프레임에 걸쳐 이동 → 놓음 (egui 의 드래그 판정을 그대로 거친다)
                Step::Drag(x0, y0, x1, y1) => {
                    let (a, b) = (egui::pos2(x0, y0), egui::pos2(x1, y1));
                    let btn = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: none };
                    self.injected.push_back(vec![egui::Event::PointerMoved(a)]);
                    self.injected.push_back(vec![btn(a, true)]);
                    for k in 1..=8 {
                        let t = k as f32 / 8.0;
                        self.injected.push_back(vec![egui::Event::PointerMoved(a + (b - a) * t)]);
                    }
                    self.injected.push_back(vec![egui::Event::PointerMoved(b)]);
                    self.injected.push_back(vec![btn(b, false)]);
                    self.injected.push_back(vec![]);
                    self.request_redraw();
                    return Some(Instant::now() + Duration::from_millis(16));
                }
                //#endregion
                Step::Type(text) => {
                    self.injected.push_back(vec![egui::Event::Text(text)]);
                    self.injected.push_back(vec![]);
                    self.request_redraw();
                    return Some(Instant::now() + Duration::from_millis(16));
                }
                //#region 수식키 클릭 — egui 는 수식키를 ModifiersChanged 이벤트로 안다
                Step::ModClick(x, y, cmd, shift) => {
                    let pos = egui::pos2(x, y);
                    let mods = egui::Modifiers { command: cmd, mac_cmd: cmd, shift, ..Default::default() };
                    let press = |p: bool| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: p, modifiers: mods };
                    self.injected.push_back(vec![egui::Event::ModifiersChanged(mods), egui::Event::PointerMoved(pos)]);
                    self.injected.push_back(vec![press(true), press(false)]);
                    self.injected.push_back(vec![egui::Event::ModifiersChanged(none)]);
                    if let Some(g) = &self.gfx {
                        g.window.request_redraw();
                    }
                    return Some(Instant::now() + Duration::from_millis(16));
                }
                //#endregion
                Step::Scroll(x, y, dy) => {
                    let pos = egui::pos2(x, y);
                    self.injected.push_back(vec![egui::Event::PointerMoved(pos)]);
                    self.injected.push_back(vec![egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, dy),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: none,
                    }]);
                    self.injected.push_back(vec![]);
                    if let Some(g) = &self.gfx {
                        g.window.request_redraw();
                    }
                    return Some(Instant::now() + Duration::from_millis(16));
                }
                Step::Key(name) => {
                // "cmd+s" = ⌘ 를 누른 채
                let (mods, name) = match name.strip_prefix("cmd+") {
                    Some(b) => (ModifiersState::SUPER, b.to_string()),
                    None => (ModifiersState::empty(), name),
                };
                match input::named(&name) {
                    // 실제 입력과 같은 관문: 글 입력 칸에 포커스가 있으면 키는 egui 몫 (앱 단축키로 새지 않음)
                    Some(_) if self.gfx.as_ref().and_then(|g| g.egui.as_ref()).is_some_and(|(ctx, _)| ctx.text_edit_focused()) => {
                        if let Some(key) = egui::Key::from_name(&name).or(match name.as_str() {
                            "backspace" => Some(egui::Key::Backspace),
                            "enter" => Some(egui::Key::Enter),
                            "escape" | "esc" => Some(egui::Key::Escape),
                            _ => None,
                        }) {
                            let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: none };
                            self.injected.push_back(vec![ev(true), ev(false)]);
                            self.injected.push_back(vec![]);
                            if let Some(g) = &self.gfx {
                                g.window.request_redraw();
                            }
                        }
                        log::info!("자가 시험: 키 {name} → 글 입력 칸 (단축키 아님)");
                        return Some(Instant::now() + Duration::from_millis(16));
                    }
                    Some(k) => {
                        let fullscreen = self.gfx.as_ref().is_some_and(|g| g.window.fullscreen().is_some());
                        if let Some(a) = input::key_action(&k, mods, fullscreen, self.viewer.mode, self.viewer.kind()) {
                            self.handle(a, el);
                        }
                    }
                    None => log::warn!("자가 시험: 알 수 없는 키 {name}"),
                }
                }
                Step::WaitScan(ms) => {
                    self.wait_scan_until = Some(Instant::now() + Duration::from_millis(ms));
                    return Some(Instant::now() + Duration::from_millis(30));
                }
                Step::WaitThumbs(ms) => {
                    self.wait_thumbs_until = Some(Instant::now() + Duration::from_millis(ms));
                    return Some(Instant::now() + Duration::from_millis(30));
                }
                Step::Wait(_) => {}
            }
        }
    }

    fn switch_renderer(&mut self, level: RendererLevel) {
        let Some(g) = &mut self.gfx else { return };
        let window = g.window.clone();
        // 기존 렌더러(표면)를 먼저 내려놓는다
        let placeholder: Box<dyn Renderer> = Box::new(iv_render_cpu::offscreen([1, 1]));
        drop(std::mem::replace(&mut g.renderer, placeholder));
        let (r, lv) = renderer::create(&window, level);
        g.renderer = r;
        g.level = lv;
        g.egui = make_egui(&window, g.renderer.as_ref());
        // 새 egui 컨텍스트 → 썸네일 텍스처·스크롤 위치가 사라진다: 다시 올리고 선택 항목으로 스크롤
        let sel = (!self.viewer.library.is_empty()).then(|| self.viewer.library.index());
        self.browser.reset_context(sel);
        self.last_requested.clear();
        self.viewer.config.max_texture_side = g.renderer.max_texture_side();
        self.set_browse_available();
        // 텍스처 한도가 바뀌었으니 필요한 해상도를 다시 알린다
        self.loader.set_window(&self.viewer.window());
        self.update_title();
        window.request_redraw();
    }

    /// UI 를 못 그리는 렌더러면 브라우저 대신 전체 보기로만
    fn set_browse_available(&mut self) {
        let full = self.gfx.as_ref().is_some_and(|g| g.renderer.ui_capability() == UiCapability::Full);
        self.viewer.browse_available = full;
        if !full && self.viewer.mode == Mode::Browse {
            self.viewer.mode = Mode::View;
            self.thumbs.set_paused(true);
        }
    }
}

fn make_egui(window: &Window, r: &dyn Renderer) -> Option<(egui::Context, egui_winit::State)> {
    if r.ui_capability() != UiCapability::Full {
        return None;
    }
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());
    iv_ui::install_fonts(&ctx);
    iv_ui::theme::install(&ctx);
    let state = egui_winit::State::new(
        ctx.clone(),
        egui::ViewportId::ROOT,
        window,
        Some(window.scale_factor() as f32),
        None,
        Some(r.max_texture_side() as usize),
    );
    Some((ctx, state))
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        // 자가 시험 창은 포커스를 빼앗지 않는다 (사용자가 다른 앱에서 치는 키가 들어오지 않게)
        let attrs = Window::default_attributes().with_title(APP).with_inner_size(LogicalSize::new(1280.0, 820.0)).with_active(self.selftest.is_none());
        let window = Arc::new(el.create_window(attrs).expect("창 생성 실패"));
        let (renderer, level) = renderer::create(&window, self.args.renderer);
        let egui = make_egui(&window, renderer.as_ref());
        self.viewer.config.max_texture_side = renderer.max_texture_side();
        self.viewer.config.display_side = window.current_monitor().map(|m| m.size().width.max(m.size().height));
        let size = window.inner_size();
        let _ = self.viewer.set_screen([size.width as f32, size.height as f32]);
        self.gfx = Some(Gfx { window, renderer, level, egui });
        self.set_browse_available();
        if let Some(p) = self.pending_open.take() {
            let fx = self.viewer.open(&p, &self.loader);
            self.apply(fx, el);
        }
        self.update_title();
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::LoaderWake => {
                let mut fx = Effects::default();
                for e in self.loader.drain() {
                    fx = fx.merge(self.viewer.on_load_event(e));
                }
                self.apply(fx, el);
            }
            UserEvent::ScanWake => {
                self.tree_scan_changed = true;
                //#region 바뀐 폴더 반영 — 트리는 그 폴더 하위 목록만 다시 읽고, 열린 폴더면 목록이 달라졌을 때만 새로고침
                let changed = self.scanner.take_changed();
                let open_dir = self.viewer.library.dir().map(Path::to_path_buf);
                for d in &changed {
                    self.tree.invalidate(d);
                    if open_dir.as_deref() == Some(d.as_path()) {
                        self.dir_dirty = true;
                    }
                }
                if self.dir_dirty && self.viewer.mode == Mode::Browse {
                    self.dir_dirty = false;
                    let fx = self.viewer.refresh_if_changed(&self.loader);
                    self.apply(fx, el);
                }
                //#endregion
                if self.viewer.mode == Mode::Browse {
                    if let Some(g) = &self.gfx {
                        g.window.request_redraw();
                    }
                }
            }
            UserEvent::CodecWake => self.on_codec_wake(el),
            UserEvent::DocWake => self.pump_pdf(),
            UserEvent::ThumbWake => {
                // egui 가 없어도 반드시 drain (pending 플래그 리셋)
                let evs = self.thumbs.drain();
                let mut any = false;
                for e in evs {
                    match e {
                        ThumbEvent::Ready(t) => {
                            if self.viewer.library.position(EntryId(t.key)).is_some() {
                                self.browser.textures.push(t.key, t.image, t.full_size);
                                any = true;
                            }
                        }
                        ThumbEvent::Failed { key, error } => {
                            any |= self.thumb_failed.insert(key);
                            // 엔진이 없어 못 만든 썸네일 — 설치를 권하거나(동의했으면) 바로 설치
                            if error.contains(iv_decode_video::NEED_ENGINE) && self.viewer.mode == Mode::Browse {
                                self.need_codec_engine();
                            }
                        }
                    }
                }
                if any && self.viewer.mode == Mode::Browse {
                    if let Some(g) = &self.gfx {
                        g.window.request_redraw();
                    }
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        //#region 자가 시험 중엔 실제 키보드·마우스를 무시 — 창이 포커스를 가져가 사용자의 타자(⌫ 등)가 새어 들어오지 않게
        if self.selftest.is_some()
            && matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::ModifiersChanged(_) | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } | WindowEvent::PinchGesture { .. } | WindowEvent::CursorMoved { .. } | WindowEvent::DroppedFile(_))
        {
            log::debug!("자가 시험: 실제 입력 무시 {event:?}");
            return;
        }
        //#endregion
        // 버튼 놓기·커서 이탈은 egui 가 소비해도 드래그를 끝낸다 (드래그가 붙어 버리는 것 방지)
        if matches!(event, WindowEvent::MouseInput { state: ElementState::Released, .. } | WindowEvent::CursorLeft { .. } | WindowEvent::Focused(false)) {
            self.dragging = None;
        }
        // 글 문서를 보는 중이면 마우스(휠 스크롤·끌어 선택)는 egui 몫 — 앱의 확대·이동 매핑을 쓰지 않는다
        let browse = self.viewer.mode == Mode::Browse || self.text_doc.is_some();
        // egui 가 먼저 본다 (버튼 위 클릭 등은 egui 몫)
        if let Some(g) = &mut self.gfx {
            // RedrawRequested 를 egui 에 넘기면 repaint=true 를 돌려줘 무한 다시 그리기가 된다 (대기 중 CPU 12%)
            if let (Some((ctx, state)), false) = (&mut g.egui, matches!(event, WindowEvent::RedrawRequested)) {
                let r = state.on_window_event(&g.window, &event);
                if r.repaint {
                    g.window.request_redraw();
                }
                // 키는 글 입력 중일 때만 egui 몫 (위젯 포커스가 단축키를 삼키지 않게)
                let is_key = matches!(event, WindowEvent::KeyboardInput { .. });
                if r.consumed && (!is_key || ctx.text_edit_focused()) {
                    return;
                }
            }
        }
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => {
                if let Some(g) = &mut self.gfx {
                    g.renderer.resize([s.width, s.height]);
                }
                let side = self.monitor_side();
                let fx = self.viewer.set_display_side(side).merge(self.viewer.set_screen([s.width as f32, s.height as f32]));
                self.apply(fx, el);
            }
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let side = self.monitor_side();
                let fx = self.viewer.set_display_side(side);
                self.apply(fx, el);
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            // 이름 입력 창이 열려 있으면 앱 단축키를 쓰지 않는다 (Tab 으로 입력 칸을 벗어나도 ⌫·Delete 가 새지 않게)
            WindowEvent::KeyboardInput { .. } if self.browser.dialog.is_some() => {}
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let fullscreen = self.gfx.as_ref().is_some_and(|g| g.window.fullscreen().is_some());
                if let Some(a) = input::key_action(&event.logical_key, self.mods, fullscreen, self.viewer.mode, self.viewer.kind()) {
                    // 키를 누르고 있어도 휴지통은 한 번만 (반복 입력이 다음 장을 연달아 지우지 않게)
                    if !(event.repeat && matches!(a, Action::Trash | Action::SaveRotation)) {
                        self.handle(a, el);
                    }
                }
            }
            // 브라우저에서는 마우스를 egui(격자 스크롤·클릭)가 맡는다 — 앱의 확대·이동 매핑은 전체 보기에서만
            WindowEvent::CursorMoved { position, .. } if browse => self.cursor = position,
            WindowEvent::MouseWheel { .. } | WindowEvent::PinchGesture { .. } | WindowEvent::MouseInput { .. } if browse => {}
            WindowEvent::MouseWheel { delta, .. } => {
                let anchor = [self.cursor.x as f32, self.cursor.y as f32];
                // 휠은 언제나 커서 기준 확대/축소. 맥의 부드러운 스크롤 마우스(Magic Mouse 등)는 픽셀 단위로 오므로
                // 그것도 확대로 받는다 — 이동은 끌기로 한다.
                let factor = match delta {
                    MouseScrollDelta::LineDelta(_, y) => 1.15f32.powf(y),
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32 * 0.003).exp().clamp(0.5, 2.0),
                };
                if factor != 1.0 {
                    self.handle(Action::ZoomBy { factor, anchor }, el);
                }
            }
            WindowEvent::PinchGesture { delta, .. } => {
                let anchor = [self.cursor.x as f32, self.cursor.y as f32];
                self.handle(Action::ZoomBy { factor: (1.0 + delta as f32).clamp(0.5, 2.0), anchor }, el);
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => match state {
                ElementState::Pressed => {
                    let now = Instant::now();
                    let double = self.last_click.is_some_and(|t| now.duration_since(t) < Duration::from_millis(350));
                    self.last_click = Some(now);
                    if double {
                        self.last_click = None;
                        let anchor = [self.cursor.x as f32, self.cursor.y as f32];
                        self.handle(Action::ToggleFitActual { anchor }, el);
                    } else {
                        self.dragging = Some(self.cursor);
                    }
                }
                ElementState::Released => self.dragging = None,
            },
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(prev) = self.dragging {
                    let d = [(position.x - prev.x) as f32, (position.y - prev.y) as f32];
                    self.dragging = Some(position);
                    self.handle(Action::Pan(d), el);
                }
                self.cursor = position;
            }
            WindowEvent::DroppedFile(p) => self.handle(Action::Open(p), el),
            WindowEvent::RedrawRequested => self.frame(el, None),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        // 1) egui 가 요청한 다시 그리기 시각이 지났으면 그리고 비운다 (지난 시각으로 WaitUntil 하면 바쁜 대기)
        if let Some(at) = self.repaint_at {
            if Instant::now() >= at {
                self.repaint_at = None;
                if let Some(g) = &self.gfx {
                    g.window.request_redraw();
                }
            }
        }
        // 2) 자가 시험 진행
        let test_at = if self.selftest.is_some() && self.gfx.is_some() { self.run_selftest(el) } else { None };
        // 3) 비디오 프레임 (재생 중에만 깨어난다 — 멈추면 대기 CPU 0)
        let video_at = self.pump_video();
        self.pump_pdf();
        // 4) 미래 시각 중 가장 이른 것까지 대기, 없으면 이벤트가 올 때까지
        let now = Instant::now();
        let next = [self.repaint_at, test_at, video_at].into_iter().flatten().filter(|t| *t > now).min();
        el.set_control_flow(match next {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

/// 파일에 적힌 방향에서 시계 방향 90° × `quarters` 더 돌린 방향으로 저장 (방향 태그만 — 픽셀 재압축 없음)
fn rotate_file(path: &std::path::Path, quarters: u8) -> Result<PathBuf, String> {
    let current = iv_rotate::read_orientation(path).ok_or("사진 방향을 읽지 못함")?;
    iv_rotate::save_orientation(path, current.rotated_quarters(quarters))?;
    Ok(path.to_path_buf())
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// 자가 시험 중엔 시험용 클립보드 파일만 (사용자 클립보드를 덮지 않게)
fn clip_ok(selftest: bool) -> Result<(), String> {
    if selftest && std::env::var_os("IV_CLIPBOARD_FILE").is_none() {
        return Err("자가 시험: IV_CLIPBOARD_FILE 없이 클립보드를 쓰지 않음".into());
    }
    Ok(())
}

//#region 종류·필터 → 스캔 묶음 비트
/// 지금 종류의 묶음 전부
fn kind_mask(groups: &[iv_core::formats::FormatGroup], v: &ViewerState) -> u64 {
    let k = v.kind();
    iv_core::formats::group_mask(groups, |g| g.kind == k)
}

/// 트리 숨김·숫자에 쓰는 비트: 지금 종류 ∩ 필터 (필터 없으면 종류 전부 — 다른 종류는 빠진다)
fn tree_mask(groups: &[iv_core::formats::FormatGroup], v: &ViewerState) -> u64 {
    let k = v.kind();
    match v.filter() {
        Some(f) => iv_core::formats::group_mask(groups, |g| g.kind == k && g.exts.iter().any(|e| f.contains(e))),
        None => kind_mask(groups, v),
    }
}
//#endregion

/// 변환본 캐시가 한도(10GB)를 넘으면 오래 안 쓴 것부터 지운다 — 백그라운드 (`keep` = 지금 열 것)
fn prune_video_cache(keep: Option<PathBuf>) {
    let Some(dir) = iv_thumbs::default_cache_dir().map(|d| d.join("video-v1")) else { return };
    let _ = std::thread::Builder::new().name("iv-video-cache-prune".into()).spawn(move || {
        iv_ffmpeg::prune_cache(&dir, iv_ffmpeg::CACHE_LIMIT, iv_ffmpeg::PRUNE_TO, keep.as_deref());
    });
}
