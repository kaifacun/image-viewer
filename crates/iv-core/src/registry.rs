use std::sync::Arc;

use crate::{CancelToken, DecodeError, Decoder, Frame, ImageInfo, ImageSource, Result, SizeHint};

/// 등록된 디코더 모음. probe 우선순위 순으로 시도하고,
/// `allows_fallback()` 인 오류면 다음 디코더로 넘어간다.
#[derive(Default, Clone)]
pub struct DecoderRegistry {
    decoders: Vec<Arc<dyn Decoder>>,
}

/// 매직바이트 판별에 읽는 앞부분 크기
const HEAD_LEN: usize = 64;

impl DecoderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, d: impl Decoder + 'static) -> &mut Self {
        self.decoders.push(Arc::new(d));
        self
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.decoders.iter().map(|d| d.name()).collect()
    }

    /// 등록된 디코더들이 아는 확장자 합집합 (폴더 스캔 필터용)
    pub fn known_extensions(&self) -> std::collections::HashSet<&'static str> {
        self.decoders.iter().flat_map(|d| d.extensions().iter().copied()).collect()
    }

    /// 이 출처를 다룰 수 있는 디코더 목록 (우선순위 내림차순)
    pub fn candidates(&self, src: &ImageSource) -> Result<Vec<Arc<dyn Decoder>>> {
        let head = src.read_head(HEAD_LEN)?;
        let mut list: Vec<(i32, Arc<dyn Decoder>)> =
            self.decoders.iter().filter_map(|d| d.probe(src, &head).map(|p| (p, d.clone()))).collect();
        list.sort_by(|a, b| b.0.cmp(&a.0));
        if list.is_empty() {
            return Err(DecodeError::Unsupported(src.display_name()));
        }
        Ok(list.into_iter().map(|(_, d)| d).collect())
    }

    /// 후보들을 차례로 시도하는 공통 루프
    fn try_each<T>(&self, src: &ImageSource, mut f: impl FnMut(&dyn Decoder) -> Result<T>) -> Result<T> {
        let mut last = None;
        for d in self.candidates(src)? {
            match f(d.as_ref()) {
                Ok(v) => return Ok(v),
                Err(e) if e.allows_fallback() => {
                    log::debug!("{} 실패, 다음 디코더 시도: {e}", d.name());
                    last = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| DecodeError::Unsupported(src.display_name())))
    }

    pub fn info(&self, src: &ImageSource) -> Result<ImageInfo> {
        self.try_each(src, |d| d.info(src))
    }

    pub fn decode(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Frame> {
        self.try_each(src, |d| d.decode(src, hint, cancel))
    }

    /// 미리보기 — 어느 디코더도 없으면 Ok(None)
    pub fn preview(&self, src: &ImageSource, hint: SizeHint, cancel: &CancelToken) -> Result<Option<Frame>> {
        for d in self.candidates(src)? {
            if !d.capabilities().preview {
                continue;
            }
            match d.preview(src, hint, cancel) {
                Ok(Some(f)) => return Ok(Some(f)),
                Ok(None) => {}
                Err(DecodeError::Cancelled) => return Err(DecodeError::Cancelled),
                Err(e) => log::debug!("{} 미리보기 실패: {e}", d.name()),
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(&'static str, i32, fn() -> Result<Frame>);
    impl Decoder for Fake {
        fn name(&self) -> &'static str {
            self.0
        }
        fn probe(&self, _: &ImageSource, _: &[u8]) -> Option<i32> {
            Some(self.1)
        }
        fn info(&self, _: &ImageSource) -> Result<ImageInfo> {
            Ok(ImageInfo::default())
        }
        fn decode(&self, _: &ImageSource, _: SizeHint, _: &CancelToken) -> Result<Frame> {
            (self.2)()
        }
    }

    fn src() -> ImageSource {
        // Cargo.toml 은 항상 존재하므로 read_head 가 성공한다
        ImageSource::Path(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
    }

    #[test]
    fn falls_back_on_invalid_data_by_priority() {
        let mut r = DecoderRegistry::new();
        r.register(Fake("low", 1, || Ok(Frame::rgba8(1, 1, vec![0; 4], [1, 1]))));
        r.register(Fake("high", 10, || Err(DecodeError::InvalidData("x".into()))));
        let f = r.decode(&src(), SizeHint::FULL, &CancelToken::new()).unwrap();
        assert_eq!(f.width, 1);
    }

    #[test]
    fn io_error_stops_immediately() {
        let mut r = DecoderRegistry::new();
        r.register(Fake("low", 1, || Ok(Frame::rgba8(1, 1, vec![0; 4], [1, 1]))));
        r.register(Fake("high", 10, || Err(DecodeError::Io(std::io::Error::other("x")))));
        assert!(matches!(r.decode(&src(), SizeHint::FULL, &CancelToken::new()), Err(DecodeError::Io(_))));
    }
}
