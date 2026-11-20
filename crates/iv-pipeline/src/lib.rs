//! 디코드 파이프라인.
//!
//! 앱은 "지금 무엇을 보고 있는지"만 [`Loader::set_window`] 로 알린다.
//! 파이프라인이 우선순위·취소·중복 제거·캐시를 맡고, 결과는 내부 큐에 쌓은 뒤
//! `wake` 콜백을 한 번 불러 앱이 메인 스레드에서 [`Loader::drain`] 하게 한다.
//! winit·렌더러·구체 디코더를 모른다.

mod cache;
mod loader;

pub use cache::{CacheStats, FrameCache};
pub use loader::{LoadEvent, Loaded, Loader, LoaderConfig, Quality, Want, Window};

/// 항목 식별자 (앱이 파일 지문으로 만든 값)
pub type EntryKey = u64;
