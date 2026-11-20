use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 오래 걸리는 디코드를 중도에 멈추기 위한 협조적 취소 토큰.
/// 디코더는 스캔라인·타일 단위로 [`CancelToken::is_cancelled`] 를 확인한다.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    /// 같은 토큰인가 (복제본끼리 true)
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// 취소됐으면 `Err(Cancelled)` — 디코더 안에서 `?` 로 쓴다.
    pub fn check(&self) -> crate::Result<()> {
        if self.is_cancelled() { Err(crate::DecodeError::Cancelled) } else { Ok(()) }
    }
}
