//! 비디오 재생기. 소리는 AVPlayer 가 직접 내고, 화면은 [`VideoPlayer::poll_frame`] 이 돌려주는
//! [`Frame`](iv_core::Frame) 을 앱이 렌더러 이미지 경로로 그린다 — 확대·이동·UI·자가 캡처가 사진과 똑같이 동작한다.
//!
//! 프레임은 메인 스레드에서 꺼낸다 (AVPlayer 는 메인 스레드에서 만든다). 큰 영상은 출력 크기를
//! `max_side` 로 줄여 받아 복사 비용을 막는다. 회전 표시 정보(세로 영상)는 `Frame.orientation` 으로 넘긴다.

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub use mac::VideoPlayer;

#[cfg(not(target_os = "macos"))]
mod other {
    use std::path::Path;

    /// 이 OS 에서는 아직 재생하지 않는다
    pub struct VideoPlayer;

    impl VideoPlayer {
        pub fn open(_path: &Path, _max_side: u32, _muted: bool) -> Result<Self, String> {
            Err("이 OS 에서는 아직 비디오를 재생하지 않음".into())
        }
        pub fn play(&self) {}
        pub fn pause(&self) {}
        pub fn failed(&self) -> bool {
            true
        }
        pub fn toggle(&self) {}
        pub fn is_playing(&self) -> bool {
            false
        }
        pub fn position(&self) -> f64 {
            0.0
        }
        pub fn duration(&self) -> f64 {
            0.0
        }
        pub fn seek(&self, _secs: f64) {}
        pub fn set_volume(&self, _v: f32) {}
        pub fn volume(&self) -> f32 {
            0.0
        }
        pub fn set_muted(&self, _m: bool) {}
        pub fn muted(&self) -> bool {
            true
        }
        pub fn poll_frame(&mut self) -> Option<iv_core::Frame> {
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub use other::VideoPlayer;
