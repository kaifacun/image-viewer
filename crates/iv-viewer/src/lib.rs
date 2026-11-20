//! 뷰어 상태와 사용자 동작(Action) 처리.
//!
//! 창·입력·렌더러를 모른다. 앱은 입력을 [`Action`] 으로 바꿔 [`ViewerState::handle`] 에 넘기고,
//! 돌려받은 [`Effects`] 대로 다시 그리기·파이프라인 갱신·창 조작을 한다.
//! 새 기능 = Action 추가 + 처리 분기 추가.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iv_core::formats::MediaKind;
use iv_core::{Frame, Orientation};
pub use iv_library::SortKey;
use iv_library::{Entry, EntryId, Library};
use iv_pipeline::{LoadEvent, Loader, Quality, Want, Window};
use iv_viewport::{Viewport, ZoomMode};

/// 사용자 동작. 단축키·마우스·메뉴·UI 버튼이 모두 이것을 낸다.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Next,
    Prev,
    First,
    Last,
    /// 화면 중앙 기준 확대/축소 한 단계
    ZoomIn,
    ZoomOut,
    /// 배율 곱하기, anchor = 화면 좌표 (휠·핀치)
    ZoomBy { factor: f32, anchor: [f32; 2] },
    /// 화면 픽셀만큼 이동 (드래그·트랙패드)
    Pan([f32; 2]),
    Fit,
    FitAlways,
    ActualSize,
    /// 맞춤 ↔ 1:1 전환 (더블클릭)
    ToggleFitActual { anchor: [f32; 2] },
    Open(PathBuf),
    ToggleFullscreen,
    ToggleInfo,
    Quit,
    // ── 브라우저 모드 ──
    /// 선택한 장을 전체 보기로
    EnterView,
    /// 전체 보기 → 브라우저
    ExitView,
    /// 격자에서 선택 (클릭)
    Select(usize),
    /// 선택 + 전체 보기 (더블클릭)
    Activate(usize),
    /// 격자 이동 (화살표): dx 칸, dy 줄
    GridMove { dx: isize, dy: isize },
    /// 한 화면(보이는 줄 수)씩 위·아래
    GridPage(isize),
    /// 폴더 열기 (브라우저 유지)
    OpenFolder(PathBuf),
    ParentFolder,
    Refresh,
    Sort(SortKey, bool),
    // ── 여러 장 선택·삭제 ──
    /// ⌘+클릭: 이 항목을 선택에 넣거나 뺀다
    ToggleSelect(usize),
    /// ⇧+클릭: 기준 항목부터 여기까지 선택
    RangeSelect(usize),
    /// ⇧+화살표: 선택 범위를 늘리며 이동
    GridExtend { dx: isize, dy: isize },
    /// ⌘A
    SelectAll,
    /// 빈 공간 클릭 — 선택 모두 풀기
    SelectNone,
    /// 빈 공간에서 끌어 그린 상자에 닿은 항목들. `keep` 이면 시작 때 선택에 더하고, `begin` 은 끌기의 첫 번째
    MarqueeSelect { indices: Vec<usize>, keep: bool, begin: bool },
    /// 선택한 항목(전체 보기에선 보고 있는 장)을 휴지통으로 — 실제 이동은 앱이 한다
    Trash,
    // ── 파일·폴더 작업 (실제 작업은 앱이 한다 — [`ViewerState::take_file_op`]) ──
    /// 이름 입력 대화 상자 열기 (F2 · ⇧⌘N) — 앱이 UI 에 연다
    BeginRename,
    BeginNewFolder,
    NewFolder { parent: PathBuf, name: String },
    RenameFolder { path: PathBuf, name: String },
    RenameFile { path: PathBuf, name: String },
    /// 선택한 사진들을 이 폴더로 옮긴다 (썸네일을 트리에 끌어다 놓기)
    MoveMarkedTo(PathBuf),
    // ── 회전 (전체 보기) ──
    /// 보이는 그림을 시계 방향 90° (R)
    RotateCw,
    /// 반시계 방향 90° (L · ⇧R)
    RotateCcw,
    /// 돌린 상태를 원본 파일에 저장 (⌘S) — 방향 태그만, 픽셀 재압축 없음
    SaveRotation,
    /// 브라우저: 선택한 사진들을 시계 방향 90° × n 돌려 바로 저장 (오른쪽 클릭 일괄 회전)
    RotateMarked(u8),
    // ── 클립보드 (실제 작업은 앱이) ──
    /// ⌘C — 선택한 사진(전체 보기는 보고 있는 장)을 클립보드에 (Finder 에 붙여넣기 가능)
    Copy,
    /// ⌘X — 복사 + 붙여넣을 때 옮기기
    Cut,
    /// ⌘V — 클립보드의 파일을 이 폴더(None = 열린 폴더)에
    Paste(Option<PathBuf>),
    /// ⌥⌘C — 선택한 사진의 절대 경로(없으면 열린 폴더 경로)를 글로 복사
    CopyPath,
    /// 확장자 필터 — 이 확장자의 사진만 보인다 (None·빈 목록 = 필터 없음)
    SetFilter(Option<Vec<&'static str>>),
    /// 화면 종류 바꾸기 (사진 ↔ 비디오) — 필터는 풀고 목록을 그 종류로 다시 나열
    SetKind(MediaKind),
    /// 비디오 재생 조작 (전체 보기에서) — 실제 재생기는 앱이 가진다
    Video(VideoCmd),
    /// 문서 쪽 넘기기 (전체 보기) — 쪽 상태는 앱이 가진다
    Page(PageCmd),
    /// 추가 코덱 엔진 받기에 동의 / 나중에 (앱이 처리 — 뷰어 상태와 무관)
    InstallCodecs,
    DismissCodecPrompt,
}

/// 문서 쪽 명령
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageCmd {
    Next,
    Prev,
    First,
    Last,
    /// 이 쪽으로 (0부터)
    Go(usize),
}

/// 비디오 재생 명령
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VideoCmd {
    /// 재생 ↔ 일시 정지 (Space)
    Toggle,
    /// 이 위치로 (초)
    Seek(f64),
    /// 앞뒤로 (초, ←/→)
    SeekBy(f64),
    /// 소리 크기 0~1
    Volume(f32),
    /// 소리 크기 더하기 (↑/↓)
    VolumeBy(f32),
    ToggleMute,
}

/// 앱이 실행할 파일 작업
#[derive(Clone, Debug, PartialEq)]
pub enum FileOp {
    NewFolder { parent: PathBuf, name: String },
    RenameFolder { path: PathBuf, name: String },
    RenameFile { path: PathBuf, name: String },
    /// 선택한 사진들([`ViewerState::trash_targets`] 와 같은 대상)을 옮김 — 결과는 [`ViewerState::on_moved`]
    MoveTo { dest: PathBuf },
    /// 파일을 시계 방향 90° × `quarters` 만큼 더 돌린 방향으로 저장 (성공하면 같은 경로를 돌려준다)
    Rotate { path: PathBuf, quarters: u8 },
    /// 선택한 사진들([`ViewerState::trash_targets`])을 돌려 저장 — 결과는 [`ViewerState::on_rotated`]
    RotateMarked { quarters: u8 },
    /// 클립보드의 파일을 `dest` 에 복사(잘라냈으면 옮김) — 결과는 [`ViewerState::on_pasted`]
    Paste { dest: PathBuf },
}

/// 클립보드 작업 (대상은 [`ViewerState::trash_targets`] / [`ViewerState::path_targets`])
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipOp {
    Copy,
    Cut,
    CopyPath,
}

/// 화면 모드
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 폴더 트리 + 썸네일 격자 + 미리보기
    Browse,
    /// 한 장 전체 보기
    View,
}

/// 앱(창)이 해야 할 일
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AppRequest {
    ToggleFullscreen,
    Quit,
    /// [`ViewerState::trash_targets`] 를 휴지통으로 보내고 [`ViewerState::on_trashed`] 로 알려 달라
    Trash,
    /// [`ViewerState::take_file_op`] 를 실행하고 [`ViewerState::on_file_op`] 로 알려 달라
    FileOp,
    /// 클립보드에 쓰기 — 끝나면 [`ViewerState::notice`] 로 알린다
    Clipboard(ClipOp),
    /// 비디오 재생기에 명령
    Video(VideoCmd),
    /// 문서 쪽 넘기기
    Page(PageCmd),
}

#[derive(Clone, Copy, Debug, Default)]
#[must_use]
pub struct Effects {
    pub redraw: bool,
    /// 파이프라인에 새 창(window)을 알려야 함
    pub window_changed: bool,
    pub app: Option<AppRequest>,
    /// 목록(폴더·정렬·새로고침)이 바뀜 — 앱이 썸네일 텍스처·트리를 맞춘다
    pub library_changed: bool,
    /// 선택 항목이 격자에서 보이도록 스크롤 (키보드 이동·복귀). 클릭 선택에는 내지 않는다 (더블클릭 보호).
    pub reveal: bool,
}

impl Effects {
    fn redraw() -> Self {
        Self { redraw: true, ..Default::default() }
    }
    fn nav() -> Self {
        Self { redraw: true, window_changed: true, ..Default::default() }
    }
    fn with_reveal(mut self) -> Self {
        self.reveal = true;
        self
    }
    pub fn merge(self, o: Self) -> Self {
        Self {
            redraw: self.redraw || o.redraw,
            window_changed: self.window_changed || o.window_changed,
            app: self.app.or(o.app),
            library_changed: self.library_changed || o.library_changed,
            reveal: self.reveal || o.reveal,
        }
    }
}

/// 화면에 올라가 있는 프레임
#[derive(Clone, Debug)]
pub struct Shown {
    pub key: u64,
    pub frame: Arc<Frame>,
    pub quality: Quality,
    /// 이 프레임의 디코드 시간 (캐시에서 즉시 꺼냈으면 None)
    pub decode_ms: Option<f64>,
}

pub struct ViewerConfig {
    pub prefetch_ahead: usize,
    pub prefetch_behind: usize,
    /// 기기 최대 텍스처 크기 (렌더러가 알려줌)
    pub max_texture_side: u32,
    /// 모니터 긴 변(물리 픽셀). 있으면 창 크기 대신 이것으로 디코드 → 창 크기 조절·전체 화면 때 재디코드 폭주 방지
    pub display_side: Option<u32>,
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self { prefetch_ahead: 3, prefetch_behind: 2, max_texture_side: 8192, display_side: None }
    }
}

pub struct ViewerState {
    pub library: Library,
    pub mode: Mode,
    /// 전체 보기 화면
    pub viewport: Viewport,
    /// 브라우저 미리보기 칸 (항상 맞춤)
    pub preview: Viewport,
    /// 격자 열 수·보이는 줄 수 (UI 가 매 프레임 알려줌)
    pub grid_cols: usize,
    pub grid_rows_visible: usize,
    /// 브라우저 화면을 그릴 수 있는 렌더러인가 (아니면 전체 보기로만)
    pub browse_available: bool,
    pub shown: Option<Shown>,
    pub error: Option<String>,
    pub show_info: bool,
    /// 상태줄 알림 (복사함·붙여넣음 등). 다른 곳으로 가면 지운다.
    pub notice: Option<String>,
    pub config: ViewerConfig,
    generation: u64,
    extensions: HashSet<&'static str>,
    /// 확장자 필터 (None = 전부). 목록을 여는 모든 곳은 [`Self::exts`] 를 쓴다.
    filter: Option<HashSet<&'static str>>,
    /// 지금 보는 종류와 종류별 확장자 (비어 있으면 종류 구분 없이 `extensions` 전부)
    kind: MediaKind,
    kinds: Vec<(MediaKind, HashSet<&'static str>)>,
    /// 브라우저에서 선택된 항목들 (정렬해도 유지되게 id 로). 평범한 이동은 현재 항목 하나로 되돌린다.
    marked: HashSet<EntryId>,
    /// ⇧ 범위 선택의 기준 항목
    anchor: Option<EntryId>,
    /// 앱이 실행할 파일 작업 (Effects 는 Copy 라 여기 둔다)
    file_op: Option<FileOp>,
    /// 선택 상자를 시작할 때의 선택 (⌘/⇧ 로 더할 때 바탕)
    marquee_base: HashSet<EntryId>,
    /// 전체 보기에서 사용자가 더 돌린 양 (시계 방향 90° 단위, 0~3). 파일은 저장 전까지 그대로.
    /// 프레임은 파이프라인이 언제든 바꿔 넣으므로 회전은 여기 따로 두고 `show` 한 곳에서 입힌다.
    turns: u8,
    /// 화면 프레임의 원래(파일) 방향 — 회전을 다시 입힐 때 바탕
    shown_base: Orientation,
    /// 재생 중인 비디오 항목 — 이 항목은 파이프라인 프레임(포스터)이 재생 프레임을 덮지 않게 한다
    live: Option<u64>,
}

const ZOOM_STEP: f32 = 1.25;

impl ViewerState {
    pub fn new(screen: [f32; 2], extensions: HashSet<&'static str>, config: ViewerConfig) -> Self {
        Self {
            library: Library::default(),
            mode: Mode::Browse,
            viewport: Viewport::new(screen),
            preview: Viewport::new([1.0, 1.0]),
            grid_cols: 1,
            grid_rows_visible: 1,
            browse_available: true,
            shown: None,
            error: None,
            show_info: false,
            notice: None,
            config,
            generation: 0,
            extensions,
            filter: None,
            kind: MediaKind::Image,
            kinds: Vec::new(),
            marked: HashSet::new(),
            anchor: None,
            file_op: None,
            marquee_base: HashSet::new(),
            turns: 0,
            shown_base: Orientation::Normal,
            live: None,
        }
    }

    /// 목록에 넣을 확장자: 지금 종류의 확장자 (∩ 필터)
    fn exts(&self) -> HashSet<&'static str> {
        let base = self.kinds.iter().find(|(k, _)| *k == self.kind).map_or(&self.extensions, |(_, e)| e);
        match &self.filter {
            Some(f) => base.intersection(f).copied().collect(),
            None => base.clone(),
        }
    }

    /// 종류별 확장자를 알려 준다 (앱이 시작할 때 한 번)
    pub fn set_kinds(&mut self, kinds: Vec<(MediaKind, HashSet<&'static str>)>) {
        self.kinds = kinds;
    }

    pub fn kind(&self) -> MediaKind {
        self.kind
    }

    /// 확장자의 종류
    fn kind_of_path(&self, p: &Path) -> Option<MediaKind> {
        let ext = p.extension()?.to_str()?.to_ascii_lowercase();
        self.kinds.iter().find(|(_, e)| e.contains(ext.as_str())).map(|(k, _)| *k)
    }

    /// 지금 걸린 확장자 필터
    pub fn filter(&self) -> Option<&HashSet<&'static str>> {
        self.filter.as_ref()
    }

    pub fn current(&self) -> Option<&Entry> {
        self.library.current()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn screen_side(&self) -> u32 {
        let window = self.viewport.screen[0].max(self.viewport.screen[1]).ceil() as u32;
        let side = self.config.display_side.map_or(window, |d| d.max(window));
        side.clamp(64, self.config.max_texture_side)
    }

    /// 모니터가 바뀌었을 때 (창을 다른 화면으로 옮김)
    pub fn set_display_side(&mut self, side: Option<u32>) -> Effects {
        if self.config.display_side == side {
            return Effects::default();
        }
        self.config.display_side = side;
        Effects::nav()
    }

    /// 현재 장에 필요한 해상도 — 화면 크기, 확대 시 2배씩 계단(작업 남발 방지), 텍스처 한도로 제한
    fn current_want(&self) -> u32 {
        let base = self.screen_side();
        let Some(s) = &self.shown else { return base };
        if s.key != self.current_key().unwrap_or(0) {
            return base;
        }
        let needed = self.viewport.needed_max_side();
        let mut want = base;
        while want < needed {
            want = want.saturating_mul(2);
        }
        let full = s.frame.full_size[0].max(s.frame.full_size[1]);
        want.min(full).min(self.config.max_texture_side).max(base.min(full))
    }

    fn current_key(&self) -> Option<u64> {
        self.library.current().map(|e| e.id.0)
    }

    /// 파이프라인에 알릴 창
    pub fn window(&self) -> Window {
        // 브라우저에서는 선택 장만 (썸네일 생성과 경합하지 않게). 이웃은 전체 보기에 들어갈 때.
        let (na, nb) = match self.mode {
            Mode::View => (self.config.prefetch_ahead, self.config.prefetch_behind),
            Mode::Browse => (0, 0),
        };
        let (ahead, behind) = self.library.neighbors(na, nb);
        let conv = |v: Vec<&Entry>| v.into_iter().map(|e| (e.id.0, e.source.clone())).collect();
        Window {
            generation: self.generation,
            current: self.library.current().map(|e| (e.id.0, e.source.clone())),
            current_want: Want::Side(self.current_want()),
            ahead: conv(ahead),
            behind: conv(behind),
            neighbor_side: self.screen_side(),
        }
    }

    /// 파일이면 전체 보기, 폴더면 브라우저로 연다.
    pub fn open(&mut self, path: &Path, loader: &Loader) -> Effects {
        let is_dir = path.is_dir();
        // 파일을 직접 열면 그 파일의 종류로 바꾼다 (비디오 파일 → 비디오 모드)
        if !is_dir {
            if let Some(k) = self.kind_of_path(path).filter(|k| *k != self.kind) {
                self.kind = k;
                self.filter = None;
            }
        }
        let mode = if is_dir && self.browse_available { Mode::Browse } else { Mode::View };
        match Library::open(path, &self.exts()) {
            Ok(lib) => {
                self.library = lib;
                self.mode = mode;
                let mut fx = self.after_navigation(loader).with_reveal();
                fx.library_changed = true;
                fx
            }
            Err(e) => {
                self.error = Some(format!("열 수 없음: {} ({e})", path.display()));
                // 폴더를 못 읽음(권한·사라짐) 또는 없는 경로: 브라우저면 빈 목록으로
                if (is_dir || !path.exists()) && self.mode == Mode::Browse && self.browse_available {
                    // 트리는 새 폴더를 가리키는데 격자가 옛 폴더를 보이지 않게 비운다
                    self.library = Library::empty_at(path.to_path_buf());
                    self.marked.clear();
                    self.anchor = None;
                    self.shown = None;
                    self.generation += 1;
                    let mut fx = Effects::nav();
                    fx.library_changed = true;
                    return fx;
                }
                Effects::redraw()
            }
        }
    }

    /// 같은 폴더를 다시 읽는다 (선택·정렬 유지)
    fn refresh(&mut self, loader: &Loader) -> Effects {
        let Some(dir) = self.library.dir().map(Path::to_path_buf) else { return Effects::default() };
        let cur = self.library.current().and_then(|e| e.path().map(Path::to_path_buf));
        let (key, asc) = self.library.sort_order();
        let Ok(mut lib) = Library::open(&dir, &self.exts()) else { return self.open(&dir, loader) };
        lib.sort(key, asc);
        if let Some(i) = cur.and_then(|c| lib.entries().iter().position(|e| e.path() == Some(c.as_path()))) {
            lib.go_to(i);
        }
        self.library = lib;
        let (marked, anchor) = (std::mem::take(&mut self.marked), self.anchor);
        let mut fx = self.after_navigation(loader).with_reveal();
        // 새로고침 뒤에도 남아 있는 항목의 선택은 유지
        let alive: HashSet<EntryId> = self.library.entries().iter().map(|e| e.id).filter(|id| marked.contains(id)).collect();
        if !alive.is_empty() {
            self.marked = alive;
            self.anchor = anchor.filter(|a| self.marked.contains(a));
        }
        fx.library_changed = true;
        fx
    }

    /// 폴더 내용이 밖에서 바뀌었을 때(감시): 목록이 실제로 달라졌을 때만 새로고침 (자기 휴지통 이동 등은 그대로)
    pub fn refresh_if_changed(&mut self, loader: &Loader) -> Effects {
        let Some(dir) = self.library.dir().map(Path::to_path_buf) else { return Effects::default() };
        let Ok(lib) = Library::open(&dir, &self.exts()) else { return self.refresh(loader) };
        let now: HashSet<EntryId> = self.library.entries().iter().map(|e| e.id).collect();
        if lib.len() == now.len() && lib.entries().iter().all(|e| now.contains(&e.id)) {
            return Effects::default();
        }
        self.refresh(loader)
    }

    /// 미리보기 칸 크기 (물리 px). 해상도 요청은 모니터 기준이라 재디코드를 부르지 않는다.
    pub fn set_preview_screen(&mut self, size: [f32; 2]) -> Effects {
        if self.preview.screen == size {
            return Effects::default();
        }
        self.preview.set_screen(size);
        Effects::redraw()
    }

    pub fn set_screen(&mut self, screen: [f32; 2]) -> Effects {
        let before = self.current_want();
        self.viewport.set_screen(screen);
        Effects { redraw: true, window_changed: self.current_want() != before, ..Default::default() }
    }

    /// 넘긴 직후: 캐시에 있으면 즉시 표시 (0ms 체감)
    fn after_navigation(&mut self, loader: &Loader) -> Effects {
        self.generation += 1;
        self.error = None;
        self.notice = None;
        self.shown = None;
        self.turns = 0;
        self.live = None;
        // 평범한 이동 = 현재 항목 하나만 선택
        self.marked.clear();
        self.anchor = self.current_key().map(EntryId);
        self.marked.extend(self.anchor);
        if let Some(key) = self.current_key() {
            if let Some((frame, _)) = loader.cached(key, Some(self.screen_side())) {
                let quality = if frame.is_preview { Quality::Preview } else { Quality::Decoded };
                self.show(Shown { key, frame, quality, decode_ms: None });
            }
        }
        Effects::nav()
    }

    fn show(&mut self, mut s: Shown) {
        let same_entry = self.shown.as_ref().is_some_and(|o| o.key == s.key);
        self.shown_base = s.frame.orientation;
        if self.turns != 0 {
            s.frame = Arc::new(Frame { orientation: self.shown_base.rotated_quarters(self.turns), ..(*s.frame).clone() });
        }
        let size = s.frame.display_full_size();
        self.shown = Some(s);
        if !same_entry {
            self.viewport.set_image(size);
            self.preview.set_image(size);
        }
    }

    //#region 비디오 — 재생 프레임 받기
    /// 재생기가 꺼낸 새 프레임을 화면에 (전체 보기의 현재 항목일 때만). 같은 항목이면 확대·이동은 그대로.
    pub fn show_live(&mut self, key: u64, frame: Arc<Frame>) -> Effects {
        if self.mode != Mode::View || self.current_key() != Some(key) {
            return Effects::default();
        }
        self.live = Some(key);
        self.show(Shown { key, frame, quality: Quality::Decoded, decode_ms: None });
        Effects::redraw()
    }

    /// 문서 쪽: `new_page` 면 쪽 크기에 맞춰 화면 맞춤 (확대 다시 그리기는 확대·이동을 그대로)
    pub fn show_page(&mut self, key: u64, frame: Arc<Frame>, new_page: bool) -> Effects {
        let fx = self.show_live(key, frame);
        if fx.redraw && new_page {
            if let Some(size) = self.shown.as_ref().map(|s| s.frame.display_full_size()) {
                self.viewport.set_image(size);
                self.viewport.fit();
                self.preview.set_image(size);
            }
        }
        fx
    }
    //#endregion

    //#region 회전 — 화면에만 입히고, 저장은 앱이 (방향 태그만 바꿔)
    /// 저장하지 않은 회전 (시계 방향 90° 단위)
    pub fn turns(&self) -> u8 {
        self.turns
    }

    fn set_turns(&mut self, turns: u8) -> Effects {
        let turns = turns % 4;
        if turns == self.turns {
            return Effects::default();
        }
        self.turns = turns;
        if let Some(s) = self.shown.as_mut() {
            // 픽셀은 같은 Arc — 방향만 바뀐 프레임 (렌더러는 방향을 따라간다)
            s.frame = Arc::new(Frame { orientation: self.shown_base.rotated_quarters(turns), ..(*s.frame).clone() });
            let size = s.frame.display_full_size();
            self.viewport.set_image(size);
            self.preview.set_image(size);
            self.viewport.fit();
        }
        Effects::redraw()
    }

    fn rotate(&mut self, delta: u8) -> Effects {
        // 회전 저장은 사진 방향 태그 — 비디오는 돌리지 않는다
        if self.mode != Mode::View || self.library.current().is_none() || self.kind != MediaKind::Image {
            return Effects::default();
        }
        self.set_turns(self.turns + delta)
    }
    //#endregion

    pub fn handle(&mut self, action: Action, loader: &Loader) -> Effects {
        let center = [self.viewport.screen[0] / 2.0, self.viewport.screen[1] / 2.0];
        // 확대·이동은 전체 보기에서만
        if self.mode == Mode::Browse
            && matches!(action, Action::ZoomIn | Action::ZoomOut | Action::ZoomBy { .. } | Action::Pan(_) | Action::Fit | Action::FitAlways | Action::ActualSize | Action::ToggleFitActual { .. })
        {
            return Effects::default();
        }
        match action {
            Action::EnterView => self.enter_view(),
            Action::ExitView => {
                if self.mode == Mode::Browse || !self.browse_available {
                    return Effects::default();
                }
                // 저장하지 않은 회전은 버린다 (목록의 썸네일은 파일 그대로)
                let _ = self.set_turns(0);
                self.mode = Mode::Browse;
                self.live = None;
                Effects::nav().with_reveal()
            }
            Action::Select(i) => self.select_only(i, loader),
            Action::Activate(i) => {
                let fx = self.select_only(i, loader);
                fx.merge(self.enter_view())
            }
            Action::GridMove { dx, dy } => {
                let cols = self.grid_cols.max(1) as isize;
                self.grid_step(dx + dy * cols, loader)
            }
            Action::GridPage(d) => {
                let cols = self.grid_cols.max(1) as isize;
                let rows = self.grid_rows_visible.max(1) as isize;
                self.grid_step(d * rows * cols, loader)
            }
            Action::OpenFolder(p) => {
                if self.library.dir() == Some(p.as_path()) && self.mode == Mode::Browse {
                    return Effects::default();
                }
                self.mode = Mode::Browse;
                self.open(&p, loader)
            }
            Action::ParentFolder => match self.library.dir().and_then(Path::parent).map(Path::to_path_buf) {
                Some(p) => {
                    self.mode = Mode::Browse;
                    self.open(&p, loader)
                }
                None => Effects::default(),
            },
            Action::Refresh => self.refresh(loader),
            Action::Sort(k, asc) => {
                self.library.sort(k, asc);
                let mut fx = Effects::nav().with_reveal();
                fx.library_changed = true;
                fx
            }
            Action::Next => self.step(1, loader).with_reveal(),
            Action::Prev => self.step(-1, loader).with_reveal(),
            Action::First => self.go(0, loader).with_reveal(),
            Action::Last => self.go(self.library.len().saturating_sub(1), loader).with_reveal(),
            Action::ZoomIn => self.zoom(|v| v.zoom_by(ZOOM_STEP, center)),
            Action::ZoomOut => self.zoom(|v| v.zoom_by(1.0 / ZOOM_STEP, center)),
            Action::ZoomBy { factor, anchor } => self.zoom(|v| v.zoom_by(factor, anchor)),
            Action::Pan(d) => {
                if self.shown.is_none() {
                    return Effects::default();
                }
                if self.viewport.pan_by(d) { Effects::redraw() } else { Effects::default() }
            }
            Action::Fit => self.zoom(|v| v.fit()),
            Action::FitAlways => self.zoom(|v| v.fit_always()),
            Action::ActualSize => self.zoom(|v| v.actual_size()),
            Action::ToggleFitActual { anchor } => self.zoom(|v| {
                if v.mode == ZoomMode::Fixed || (v.scale - 1.0).abs() < 1e-3 {
                    v.fit()
                } else {
                    v.zoom_to(1.0, anchor)
                }
            }),
            Action::Open(p) => self.open(&p, loader),
            Action::ToggleInfo => {
                self.show_info = !self.show_info;
                Effects::redraw()
            }
            Action::ToggleFullscreen => Effects { app: Some(AppRequest::ToggleFullscreen), ..Default::default() },
            Action::Quit => Effects { app: Some(AppRequest::Quit), ..Default::default() },
            Action::ToggleSelect(i) if self.mode == Mode::Browse => self.toggle_select(i, loader),
            Action::RangeSelect(i) if self.mode == Mode::Browse => self.range_select(i, loader),
            Action::GridExtend { dx, dy } if self.mode == Mode::Browse && !self.library.is_empty() => {
                let cols = self.grid_cols.max(1) as isize;
                let n = self.library.len() as isize;
                let i = (self.library.index() as isize + dx + dy * cols).clamp(0, n - 1) as usize;
                self.range_select(i, loader).with_reveal()
            }
            Action::SelectAll if self.mode == Mode::Browse => {
                self.marked = self.library.entries().iter().map(|e| e.id).collect();
                Effects::redraw()
            }
            Action::SelectNone if self.mode == Mode::Browse => {
                if self.marked.is_empty() {
                    return Effects::default();
                }
                self.marked.clear();
                Effects::redraw()
            }
            Action::MarqueeSelect { indices, keep, begin } if self.mode == Mode::Browse => {
                if begin {
                    self.marquee_base = if keep { self.marked.clone() } else { HashSet::new() };
                }
                let mut m = self.marquee_base.clone();
                m.extend(indices.iter().filter_map(|&i| self.library.get(i)).map(|e| e.id));
                if m == self.marked {
                    return Effects::default();
                }
                self.marked = m;
                Effects::redraw()
            }
            Action::ToggleSelect(_) | Action::RangeSelect(_) | Action::GridExtend { .. } | Action::SelectAll | Action::SelectNone | Action::MarqueeSelect { .. } => Effects::default(),
            Action::BeginRename | Action::BeginNewFolder => Effects::default(),
            Action::NewFolder { parent, name } => self.request(FileOp::NewFolder { parent, name }),
            Action::RenameFolder { path, name } => self.request(FileOp::RenameFolder { path, name }),
            Action::RenameFile { path, name } => self.request(FileOp::RenameFile { path, name }),
            Action::MoveMarkedTo(dest) => {
                if self.mode != Mode::Browse || self.trash_targets().is_empty() || self.library.dir() == Some(dest.as_path()) {
                    return Effects::default();
                }
                self.request(FileOp::MoveTo { dest })
            }
            Action::SetKind(k) => {
                if k == self.kind {
                    return Effects::default();
                }
                self.kind = k;
                self.filter = None;
                let _ = self.set_turns(0);
                if self.browse_available {
                    self.mode = Mode::Browse;
                }
                match self.library.dir().map(Path::to_path_buf) {
                    Some(d) => {
                        let mut fx = self.open(&d, loader);
                        self.mode = if self.browse_available { Mode::Browse } else { self.mode };
                        fx.library_changed = true;
                        fx
                    }
                    None => Effects::redraw(),
                }
            }
            Action::InstallCodecs | Action::DismissCodecPrompt => Effects::default(),
            Action::Page(cmd) => {
                if self.mode != Mode::View || self.kind != MediaKind::Document {
                    return Effects::default();
                }
                Effects { app: Some(AppRequest::Page(cmd)), ..Default::default() }
            }
            Action::Video(cmd) => {
                if self.mode != Mode::View || self.kind != MediaKind::Video {
                    return Effects::default();
                }
                Effects { app: Some(AppRequest::Video(cmd)), ..Default::default() }
            }
            Action::SetFilter(f) => {
                let f: Option<HashSet<&'static str>> = f.filter(|v| !v.is_empty()).map(|v| v.into_iter().collect());
                if f == self.filter {
                    return Effects::default();
                }
                self.filter = f;
                if self.library.dir().is_none() {
                    return Effects::redraw();
                }
                // 다시 나열: 남아 있는 현재 장·선택은 지키고, 사라진 것은 뺀다
                let fx = self.refresh(loader);
                if self.library.is_empty() && self.mode == Mode::View && self.browse_available {
                    self.mode = Mode::Browse;
                }
                fx
            }
            Action::RotateMarked(q) => {
                if self.mode != Mode::Browse || q % 4 == 0 || self.kind != MediaKind::Image || self.trash_targets().is_empty() {
                    return Effects::default();
                }
                self.request(FileOp::RotateMarked { quarters: q % 4 })
            }
            Action::Copy | Action::Cut => {
                if self.trash_targets().is_empty() {
                    return Effects::default();
                }
                let op = if action == Action::Copy { ClipOp::Copy } else { ClipOp::Cut };
                Effects { app: Some(AppRequest::Clipboard(op)), ..Default::default() }
            }
            Action::CopyPath => {
                if self.path_targets().is_empty() {
                    return Effects::default();
                }
                Effects { app: Some(AppRequest::Clipboard(ClipOp::CopyPath)), ..Default::default() }
            }
            Action::Paste(dest) => match dest.or_else(|| self.library.dir().map(Path::to_path_buf)) {
                Some(dest) => self.request(FileOp::Paste { dest }),
                None => Effects::default(),
            },
            Action::RotateCw => self.rotate(1),
            Action::RotateCcw => self.rotate(3),
            Action::SaveRotation => {
                let path = self.library.current().and_then(|e| e.path()).map(Path::to_path_buf);
                match path {
                    Some(path) if self.mode == Mode::View && self.turns != 0 => self.request(FileOp::Rotate { path, quarters: self.turns }),
                    _ => Effects::default(),
                }
            }
            Action::Trash => {
                if self.trash_targets().is_empty() {
                    return Effects::default();
                }
                Effects { app: Some(AppRequest::Trash), ..Default::default() }
            }
        }
    }

    //#region 여러 장 선택
    /// 선택된 항목 id (그리기용)
    pub fn marked(&self) -> &HashSet<EntryId> {
        &self.marked
    }

    /// 클릭: 이 항목 하나만 선택 (이미 현재 항목이어도 여러 장 선택을 풀어야 한다)
    fn select_only(&mut self, i: usize, loader: &Loader) -> Effects {
        let fx = self.go(i, loader);
        if fx.redraw {
            return fx;
        }
        let Some(id) = self.library.get(i).map(|e| e.id) else { return fx };
        if self.marked.len() == 1 && self.marked.contains(&id) {
            return fx;
        }
        self.marked = HashSet::from([id]);
        self.anchor = Some(id);
        Effects::redraw()
    }

    /// 선택을 지키면서 현재 항목만 옮긴다 (미리보기가 따라감)
    fn go_keep_marks(&mut self, i: usize, loader: &Loader) -> Effects {
        let (marked, anchor) = (std::mem::take(&mut self.marked), self.anchor);
        let fx = self.go(i, loader);
        self.marked = marked;
        self.anchor = anchor;
        fx.merge(Effects::redraw())
    }

    fn toggle_select(&mut self, i: usize, loader: &Loader) -> Effects {
        let Some(id) = self.library.get(i).map(|e| e.id) else { return Effects::default() };
        let fx = self.go_keep_marks(i, loader);
        if !self.marked.remove(&id) {
            self.marked.insert(id);
        }
        self.anchor = Some(id);
        fx
    }

    fn range_select(&mut self, i: usize, loader: &Loader) -> Effects {
        if i >= self.library.len() {
            return Effects::default();
        }
        let a = self.anchor.and_then(|a| self.library.position(a)).unwrap_or(self.library.index());
        let fx = self.go_keep_marks(i, loader);
        let (lo, hi) = (a.min(i), a.max(i));
        self.marked = self.library.entries()[lo..=hi].iter().map(|e| e.id).collect();
        fx
    }
    //#endregion

    //#region 파일·폴더 작업 — 상태는 여기서, 실제 디스크 작업은 앱이
    fn request(&mut self, op: FileOp) -> Effects {
        self.file_op = Some(op);
        Effects { app: Some(AppRequest::FileOp), ..Default::default() }
    }

    pub fn take_file_op(&mut self) -> Option<FileOp> {
        self.file_op.take()
    }

    /// 작업 결과 반영. `result` = 새 경로 또는 오류.
    pub fn on_file_op(&mut self, op: &FileOp, result: &Result<PathBuf, String>, loader: &Loader) -> Effects {
        let new = match result {
            Ok(p) => p.clone(),
            Err(e) => {
                self.error = Some(e.clone());
                return Effects::redraw();
            }
        };
        match op {
            FileOp::MoveTo { .. } | FileOp::RotateMarked { .. } | FileOp::Paste { .. } => Effects::redraw(),
            FileOp::Rotate { path, .. } => {
                // 파일이 바뀌었다(mtime) → 항목을 다시 읽어 id 가 바뀐다. 화면 프레임은 이미 저장된 방향이라 그대로 둔다 (깜박임·확대 유지).
                let Some((old_id, new_id)) = self.library.rename_entry(path, new) else { return Effects::redraw() };
                if self.marked.remove(&old_id) {
                    self.marked.insert(new_id);
                }
                if self.anchor == Some(old_id) {
                    self.anchor = Some(new_id);
                }
                if let Some(s) = self.shown.as_mut().filter(|s| s.key == old_id.0) {
                    s.key = new_id.0;
                    self.shown_base = s.frame.orientation;
                }
                // 화면 프레임이 아직 없어도 회전은 이제 파일 몫 (남기면 새로 읽은 프레임에 두 번 입힌다)
                self.turns = 0;
                self.error = None;
                self.generation += 1;
                let mut fx = Effects::nav();
                fx.library_changed = true;
                fx
            }
            FileOp::NewFolder { parent, .. } => {
                // 열린 폴더 안이면 하위 폴더 수(상태줄)가 바뀌었다 — 선택은 refresh 가 지킨다
                if self.library.dir() == Some(parent.as_path()) {
                    return self.refresh(loader);
                }
                let mut fx = Effects::redraw();
                fx.library_changed = true;
                fx
            }
            FileOp::RenameFile { path, .. } => {
                let was_current = self.library.current().and_then(|e| e.path()) == Some(path.as_path());
                let Some((old_id, new_id)) = self.library.rename_entry(path, new) else { return Effects::redraw() };
                if self.marked.remove(&old_id) {
                    self.marked.insert(new_id);
                }
                if self.anchor == Some(old_id) {
                    self.anchor = Some(new_id);
                }
                let mut fx = if was_current {
                    // id 가 바뀌었으니 미리보기를 다시 (선택은 지킨다)
                    let (marked, anchor) = (std::mem::take(&mut self.marked), self.anchor);
                    let fx = self.after_navigation(loader);
                    self.marked = marked;
                    self.anchor = anchor;
                    fx
                } else {
                    Effects::redraw()
                };
                fx.library_changed = true;
                fx.with_reveal()
            }
            FileOp::RenameFolder { path, .. } => {
                // 열린 폴더(또는 그 조상)의 이름이 바뀌었으면 새 경로로 다시 연다
                let Some(dir) = self.library.dir().map(Path::to_path_buf) else { return Effects::redraw() };
                match dir.strip_prefix(path) {
                    Ok(rest) => {
                        let cur = self.library.current().map(|e| e.name.clone());
                        let (k, asc) = self.library.sort_order();
                        let mut fx = self.open(&new.join(rest), loader);
                        self.library.sort(k, asc);
                        if let Some(i) = cur.and_then(|n| self.library.entries().iter().position(|e| e.name == n)) {
                            fx = fx.merge(self.go(i, loader));
                        }
                        fx.library_changed = true;
                        fx.with_reveal()
                    }
                    Err(_) => {
                        let mut fx = Effects::redraw();
                        fx.library_changed = true;
                        fx
                    }
                }
            }
        }
    }
    //#endregion

    //#region 일괄 회전·클립보드 결과
    /// 경로 복사 대상: 선택한 사진들, 없으면 열린 폴더
    pub fn path_targets(&self) -> Vec<PathBuf> {
        let v = self.trash_targets();
        if !v.is_empty() {
            return v;
        }
        self.library.dir().map(Path::to_path_buf).into_iter().collect()
    }

    /// 앱이 클립보드에 쓴 뒤 상태줄에 알림 (오류면 오류로)
    pub fn on_clipboard(&mut self, result: Result<String, String>) -> Effects {
        match result {
            Ok(n) => self.notice = Some(n),
            Err(e) => self.error = Some(e),
        }
        Effects::redraw()
    }

    /// 돌려 저장한 파일들: 다시 읽어 id 를 바꾸고(썸네일·미리보기 새로) 선택은 지킨다
    pub fn on_rotated(&mut self, done: &[PathBuf], failures: &[String], loader: &Loader) -> Effects {
        let cur = self.current_key();
        let mut cur_changed = false;
        for p in done {
            if let Some((old, new)) = self.library.rename_entry(p, p.clone()) {
                if self.marked.remove(&old) {
                    self.marked.insert(new);
                }
                if self.anchor == Some(old) {
                    self.anchor = Some(new);
                }
                cur_changed |= Some(old.0) == cur;
            }
        }
        let mut fx = if cur_changed {
            let (marked, anchor) = (std::mem::take(&mut self.marked), self.anchor);
            let fx = self.after_navigation(loader);
            self.marked = marked;
            self.anchor = anchor;
            fx
        } else {
            Effects::redraw()
        };
        fx.library_changed = true;
        if !done.is_empty() {
            self.notice = Some(format!("{}장 돌려 저장함", done.len()));
        }
        if !failures.is_empty() {
            self.error = Some(format!("돌려 저장 못함 {}개: {}", failures.len(), failures[0]));
        }
        fx
    }

    /// 붙여넣기 결과: `created` = 새로 생긴 파일, `moved_from` = 잘라내기로 옮겨 간 원래 파일
    pub fn on_pasted(&mut self, dest: &Path, created: &[PathBuf], moved_from: &[PathBuf], failures: &[String], loader: &Loader) -> Effects {
        let mut fx = if !moved_from.is_empty() { self.on_removed(moved_from, &[], "", loader) } else { Effects::redraw() };
        if self.library.dir() == Some(dest) && !created.is_empty() {
            fx = fx.merge(self.refresh(loader));
            // 붙여넣은 사진들을 선택해 보여준다
            let ids: HashSet<EntryId> = self.library.entries().iter().filter(|e| e.path().is_some_and(|p| created.iter().any(|c| c == p))).map(|e| e.id).collect();
            if let Some(i) = self.library.entries().iter().position(|e| ids.contains(&e.id)) {
                fx = fx.merge(self.go(i, loader));
                self.marked = ids;
                fx = fx.with_reveal();
            }
        }
        fx.library_changed = true;
        let verb = if moved_from.is_empty() { "붙여넣음" } else { "옮김" };
        self.notice = (!created.is_empty()).then(|| format!("{}장 {verb}", created.len()));
        if !failures.is_empty() {
            self.error = Some(format!("붙여넣지 못함 {}개: {}", failures.len(), failures[0]));
        }
        fx
    }
    //#endregion

    //#region 휴지통
    /// 휴지통으로 보낼 파일: 전체 보기는 보고 있는 장만, 브라우저는 체크된 항목(목록 순서) — 다 풀었으면 없음
    pub fn trash_targets(&self) -> Vec<PathBuf> {
        if self.mode == Mode::View {
            return self.library.current().and_then(|e| e.path()).map(Path::to_path_buf).into_iter().collect();
        }
        self.library.entries().iter().filter(|e| self.marked.contains(&e.id)).filter_map(|e| e.path().map(Path::to_path_buf)).collect()
    }

    /// 앱이 휴지통으로 보낸 결과 반영: 성공한 것만 목록에서 빼고, 실패는 오류로 보여준다
    pub fn on_trashed(&mut self, removed: &[PathBuf], failures: &[String], loader: &Loader) -> Effects {
        self.on_removed(removed, failures, "휴지통으로 못 옮김", loader)
    }

    /// 다른 폴더로 옮긴 결과 반영 (목록에서 빠지는 것은 휴지통과 같다)
    pub fn on_moved(&mut self, moved: &[PathBuf], failures: &[String], loader: &Loader) -> Effects {
        self.on_removed(moved, failures, "옮기지 못함", loader)
    }

    fn on_removed(&mut self, removed: &[PathBuf], failures: &[String], what: &str, loader: &Loader) -> Effects {
        let ids: HashSet<EntryId> = self.library.entries().iter().filter(|e| e.path().is_some_and(|p| removed.iter().any(|r| r == p))).map(|e| e.id).collect();
        let mut fx = Effects::redraw();
        if self.library.remove_ids(&ids) {
            fx = self.after_navigation(loader).with_reveal();
            fx.library_changed = true;
            if self.library.is_empty() && self.mode == Mode::View && self.browse_available {
                self.mode = Mode::Browse;
            }
        }
        // after_navigation 이 오류를 지우므로 그 뒤에 쓴다
        if !failures.is_empty() {
            self.error = Some(format!("{what} {}개: {}", failures.len(), failures[0]));
        }
        fx
    }
    //#endregion

    fn enter_view(&mut self) -> Effects {
        if self.mode == Mode::View || self.library.current().is_none() {
            return Effects::default();
        }
        self.mode = Mode::View;
        self.viewport.fit();
        // 이웃 미리 읽기를 시작하도록 창을 다시 알린다
        Effects::nav()
    }

    fn grid_step(&mut self, d: isize, loader: &Loader) -> Effects {
        if self.library.is_empty() {
            return Effects::default();
        }
        let n = self.library.len() as isize;
        let i = (self.library.index() as isize + d).clamp(0, n - 1) as usize;
        self.go(i, loader).with_reveal()
    }

    fn step(&mut self, d: isize, loader: &Loader) -> Effects {
        if self.library.step(d) { self.after_navigation(loader) } else { Effects::default() }
    }

    fn go(&mut self, i: usize, loader: &Loader) -> Effects {
        if self.library.go_to(i) { self.after_navigation(loader) } else { Effects::default() }
    }

    fn zoom(&mut self, f: impl FnOnce(&mut Viewport)) -> Effects {
        if self.shown.is_none() {
            return Effects::default();
        }
        let before = self.current_want();
        f(&mut self.viewport);
        let after = self.current_want();
        // 더 큰 해상도가 필요해졌고 지금 프레임으로 부족하면 파이프라인에 알림
        let need_more = after > before && self.shown.as_ref().is_some_and(|s| s.frame.max_side() < after);
        Effects { redraw: true, window_changed: need_more, ..Default::default() }
    }

    /// 파이프라인 결과 반영
    pub fn on_load_event(&mut self, ev: LoadEvent) -> Effects {
        let cur = self.current_key();
        match ev {
            // 재생 중인 비디오는 재생 프레임이 화면 — 포스터로 덮지 않는다
            LoadEvent::Loaded(l) if Some(l.key) == self.live => Effects::default(),
            LoadEvent::Loaded(l) if Some(l.key) == cur => {
                let better = match &self.shown {
                    None => true,
                    Some(s) if s.key != l.key => true,
                    Some(s) => {
                        // 본 디코드는 미리보기를 대체, 본 디코드끼리는 해상도가 올라갈 때만 (단조 증가)
                        let new_is_full = l.quality == Quality::Decoded;
                        let cur_is_full = s.quality == Quality::Decoded;
                        (new_is_full && !cur_is_full) || (new_is_full && l.frame.max_side() > s.frame.max_side())
                            || (!new_is_full && !cur_is_full && l.frame.max_side() > s.frame.max_side())
                    }
                };
                if !better {
                    return Effects::default();
                }
                self.show(Shown { key: l.key, frame: l.frame, quality: l.quality, decode_ms: Some(l.decode_ms) });
                Effects::redraw()
            }
            LoadEvent::Failed { key, error, .. } if Some(key) == cur && self.shown.is_none() => {
                self.error = Some(error);
                Effects::redraw()
            }
            _ => Effects::default(),
        }
    }

    /// 브라우저 상태줄: 폴더·파일 수·용량 + 선택 항목. `selected_size` = 썸네일에서 알게 된 원본 크기.
    pub fn browse_status(&self, selected_size: Option<[u32; 2]>, renderer: &str) -> String {
        let lib = &self.library;
        let mut s = format!("폴더 {}개  ·  파일 {}개 ({})", lib.subdir_count(), lib.len(), human_bytes(lib.total_bytes()));
        if self.marked.len() > 1 {
            let bytes: u64 = lib.entries().iter().filter(|e| self.marked.contains(&e.id)).map(|e| e.len).sum();
            s += &format!("  ·  {}장 선택 ({})", self.marked.len(), human_bytes(bytes));
        }
        if let Some(e) = lib.current() {
            s += &format!("  ·  선택 {}/{}: {}", lib.index() + 1, lib.len(), e.name);
            if let Some([w, h]) = selected_size.or_else(|| self.shown.as_ref().filter(|x| Some(x.key) == self.current_key()).map(|x| x.frame.display_full_size())) {
                s += &format!("  {w}×{h}");
            }
            s += &format!("  {}", human_bytes(e.len));
        }
        if let Some(n) = &self.notice {
            s += &format!("  ·  {n}");
        }
        if let Some(err) = &self.error {
            s += &format!("  ·  오류: {err}");
        }
        s + &format!("  ·  {renderer}")
    }

    /// 상태 표시줄 문자열
    pub fn status_line(&self, renderer: &str) -> String {
        let Some(e) = self.library.current() else {
            return match &self.error {
                Some(err) => format!("{err}  ·  {renderer}"),
                None => format!("이미지를 끌어다 놓거나 파일을 여세요  ·  {renderer}"),
            };
        };
        let pos = format!("{}/{}", self.library.index() + 1, self.library.len());
        let mut s = format!("{}  ·  {pos}", e.name);
        if self.turns != 0 {
            s += &format!("  ·  {}° 돌림 (⌘S 저장)", self.turns as u32 * 90);
        }
        if let Some(sh) = &self.shown {
            let [w, h] = sh.frame.display_full_size();
            s += &format!("  ·  {w}×{h}  ·  {:.0}%", self.viewport.scale * 100.0);
            if sh.quality == Quality::Preview {
                s += "  ·  미리보기";
            }
            // 재생 중인 비디오는 디코드 시간이 의미 없다
            if self.live != Some(sh.key) {
                s += &match sh.decode_ms {
                    Some(ms) => format!("  ·  {ms:.0}ms"),
                    None => "  ·  즉시".to_string(),
                };
            }
        } else if self.error.is_none() {
            s += "  ·  불러오는 중…";
        }
        if let Some(n) = &self.notice {
            s += &format!("  ·  {n}");
        }
        if let Some(err) = &self.error {
            s += &format!("  ·  오류: {err}");
        }
        s + &format!("  ·  {renderer}")
    }
}

/// 1234567 → "1.2 MB"
pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}
