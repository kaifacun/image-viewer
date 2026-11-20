use crate::{CancelToken, Frame, ImageInfo, ImageSource, Result};

/// probe 우선순위. 높을수록 먼저 시도한다.
pub type Priority = i32;

/// 디코더가 지원하는 선택 능력
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    /// 내장 미리보기(EXIF 썸네일·RAW 프리뷰)
    pub preview: bool,
    /// 축소 디코딩 (hint 보다 크지 않게 빠르게)
    pub scaled: bool,
    /// 영역 디코딩 (타일링)
    pub region: bool,
    /// 애니메이션
    pub animation: bool,
    /// HDR / 16비트
    pub hdr: bool,
}

/// 디코드 크기 힌트
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct SizeHint {
    /// 결과 긴 변이 이 값 이상이면 충분 (None = 원본)
    pub max_side: Option<u32>,
}

impl SizeHint {
    pub const FULL: Self = Self { max_side: None };
    pub fn max_side(n: u32) -> Self {
        Self { max_side: Some(n) }
    }
}

/// 애니메이션 프레임 스트림 (이후 GIF/APNG/WebP)
pub trait FrameStream: Send {
    /// 다음 프레임과 표시 시간(ms). 끝이면 None (반복은 호출자가 다시 열기)
    fn next_frame(&mut self) -> Result<Option<(Frame, u32)>>;
}

/// 포맷 디코더 확장 지점.
///
/// 필수: `name`, `probe`, `info`, `decode`. 나머지는 기본 구현이 있어
/// 새 능력이 추가돼도 기존 디코더는 고치지 않는다.
pub trait Decoder: Send + Sync {
    fn name(&self) -> &'static str;

    fn capabilities(&self) -> Caps {
        Caps::default()
    }

    /// 이 디코더가 다루는 확장자(소문자). 폴더 스캔의 후보 축소 힌트로만 쓴다.
    fn extensions(&self) -> &'static [&'static str] {
        &[]
    }

    /// 처리 가능 여부와 우선순위. `head` = 파일 앞 64바이트 이상.
    fn probe(&self, src: &ImageSource, head: &[u8]) -> Option<Priority>;

    /// 디코드 전 메타데이터 (크기·방향)
    fn info(&self, src: &ImageSource) -> Result<ImageInfo>;

    /// 내장 미리보기. 없으면 Ok(None)
    fn preview(&self, _src: &ImageSource, _hint: SizeHint, _cancel: &CancelToken) -> Result<Option<Frame>> {
        Ok(None)
    }

    /// 본 디코드. `scaled` 능력이 있으면 hint 에 맞춰 축소 디코딩해도 된다.
    fn decode(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Frame>;

    /// 영역 디코드 (rect = 원본 좌표 [x, y, w, h], scale = 출력 배율)
    fn decode_region(
        &self,
        _src: &ImageSource,
        _rect: [u32; 4],
        _scale: f32,
        _cancel: &CancelToken,
    ) -> Result<Option<Frame>> {
        Ok(None)
    }

    fn open_animation(&self, _src: &ImageSource) -> Result<Option<Box<dyn FrameStream>>> {
        Ok(None)
    }
}
