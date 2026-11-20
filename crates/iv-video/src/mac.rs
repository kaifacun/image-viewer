use std::path::Path;

use iv_core::{Frame, Orientation};
use objc2::AnyThread;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{AVMediaTypeVideo, AVPlayer, AVPlayerItem, AVPlayerItemVideoOutput, AVPlayerTimeControlStatus, AVURLAsset};
use objc2_core_foundation::CFString;
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

/// kCVPixelFormatType_32BGRA ('BGRA')
const BGRA: u32 = 0x4247_5241;

pub struct VideoPlayer {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    output: Retained<AVPlayerItemVideoOutput>,
    orientation: Orientation,
    /// 화면에 보이는 원본 크기 (회전 반영 전, 저장 방향)
    natural: [u32; 2],
}

fn ns_key(k: &CFString) -> &NSString {
    // SAFETY: CFString 과 NSString 은 수신자 무료 브리징 (같은 객체)
    unsafe { &*(k as *const CFString as *const NSString) }
}

/// 트랙의 표시 변환(회전 행렬) → EXIF 방향
fn orientation_of(a: f64, b: f64, c: f64, d: f64) -> Orientation {
    let r = |x: f64| x.round() as i32;
    match (r(a), r(b), r(c), r(d)) {
        (0, 1, -1, 0) => Orientation::Rotate90,
        (0, -1, 1, 0) => Orientation::Rotate270,
        (-1, 0, 0, -1) => Orientation::Rotate180,
        (-1, 0, 0, 1) => Orientation::FlipH,
        (1, 0, 0, -1) => Orientation::FlipV,
        _ => Orientation::Normal,
    }
}

impl VideoPlayer {
    /// 열고 처음에 멈춘 상태. `max_side` 보다 크면 그 크기로 줄여 받는다. 메인 스레드에서만.
    #[allow(deprecated)]
    pub fn open(path: &Path, max_side: u32, muted: bool) -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("비디오는 메인 스레드에서 열어야 함")?;
        let s = path.to_str().ok_or("경로가 UTF-8 이 아님")?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(s));
        //#region 영상 트랙 — 크기·회전
        let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };
        let ty = unsafe { AVMediaTypeVideo }.ok_or("AVMediaTypeVideo 없음")?;
        let tracks = unsafe { asset.tracksWithMediaType(ty) };
        let track = tracks.firstObject().ok_or("영상 트랙이 없거나 이 형식을 재생할 수 없음")?;
        let n = unsafe { track.naturalSize() };
        let t = unsafe { track.preferredTransform() };
        let natural = [n.width.abs().round().max(1.0) as u32, n.height.abs().round().max(1.0) as u32];
        let orientation = orientation_of(t.a, t.b, t.c, t.d);
        //#endregion
        //#region 프레임 출력 — BGRA, 너무 크면 줄여서
        let long = natural[0].max(natural[1]);
        let scale = if long > max_side.max(64) { max_side.max(64) as f64 / long as f64 } else { 1.0 };
        let (ow, oh) = (((natural[0] as f64 * scale).round() as i32) & !1, ((natural[1] as f64 * scale).round() as i32) & !1);
        let keys = [ns_key(unsafe { kCVPixelBufferPixelFormatTypeKey }), ns_key(unsafe { kCVPixelBufferWidthKey }), ns_key(unsafe { kCVPixelBufferHeightKey })];
        let fmt = NSNumber::new_u32(BGRA);
        let (wn, hn) = (NSNumber::new_i32(ow.max(2)), NSNumber::new_i32(oh.max(2)));
        let vals: [&AnyObject; 3] = [&fmt, &wn, &hn];
        let attrs = NSDictionary::from_slices(&keys, &vals);
        let output = unsafe { AVPlayerItemVideoOutput::initWithPixelBufferAttributes(AVPlayerItemVideoOutput::alloc(), Some(&attrs)) };
        //#endregion
        let item = unsafe { AVPlayerItem::playerItemWithURL(&url, mtm) };
        unsafe { item.addOutput(&output) };
        let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
        unsafe { player.setMuted(muted) };
        Ok(Self { player, item, output, orientation, natural })
    }

    pub fn play(&self) {
        // 끝에 있으면 처음부터
        let d = self.duration();
        if d > 0.0 && self.position() >= d - 0.05 {
            self.seek(0.0);
        }
        unsafe { self.player.play() };
    }

    pub fn pause(&self) {
        unsafe { self.player.pause() };
    }

    pub fn toggle(&self) {
        if self.is_playing() { self.pause() } else { self.play() }
    }

    /// 재생 중 (버퍼링으로 기다리는 중 포함)
    pub fn is_playing(&self) -> bool {
        let st = unsafe { self.player.timeControlStatus() };
        st != AVPlayerTimeControlStatus::Paused
    }

    pub fn position(&self) -> f64 {
        let s = unsafe { self.item.currentTime().seconds() };
        if s.is_finite() { s.max(0.0) } else { 0.0 }
    }

    pub fn duration(&self) -> f64 {
        let s = unsafe { self.item.duration().seconds() };
        if s.is_finite() { s.max(0.0) } else { 0.0 }
    }

    /// 정확한 위치로 (키프레임 사이도)
    pub fn seek(&self, secs: f64) {
        let d = self.duration();
        let t = if d > 0.0 { secs.clamp(0.0, d) } else { secs.max(0.0) };
        let zero = unsafe { CMTime::with_seconds(0.0, 600) };
        unsafe { self.player.seekToTime_toleranceBefore_toleranceAfter(CMTime::with_seconds(t, 600), zero, zero) };
    }

    pub fn set_volume(&self, v: f32) {
        unsafe { self.player.setVolume(v.clamp(0.0, 1.0)) };
    }

    pub fn volume(&self) -> f32 {
        unsafe { self.player.volume() }
    }

    pub fn set_muted(&self, m: bool) {
        unsafe { self.player.setMuted(m) };
    }

    pub fn muted(&self) -> bool {
        unsafe { self.player.isMuted() }
    }

    /// 지금 보여 줄 새 프레임이 있으면 꺼낸다 (없으면 None — 이전 프레임을 계속 보이면 된다)
    pub fn poll_frame(&mut self) -> Option<Frame> {
        let t = unsafe { self.item.currentTime() };
        if !unsafe { self.output.hasNewPixelBufferForItemTime(t) } {
            return None;
        }
        let pb = unsafe { self.output.copyPixelBufferForItemTime_itemTimeForDisplay(t, std::ptr::null_mut()) }?;
        if unsafe { CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly) } != 0 {
            return None;
        }
        let (w, h, stride) = (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb), CVPixelBufferGetBytesPerRow(&pb));
        let base = CVPixelBufferGetBaseAddress(&pb) as *const u8;
        let frame = (!base.is_null() && w > 0 && h > 0).then(|| {
            // BGRA(줄 끝 여백 있음) → 꽉 찬 RGBA
            let mut px = vec![0u8; w * h * 4];
            for y in 0..h {
                // SAFETY: 잠근 버퍼의 한 줄 (stride ≥ w*4)
                let src = unsafe { std::slice::from_raw_parts(base.add(y * stride), w * 4) };
                let dst = &mut px[y * w * 4..(y + 1) * w * 4];
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
            Frame::rgba8(w as u32, h as u32, px, self.natural).with_orientation(self.orientation)
        });
        unsafe { CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly) };
        frame
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // 다른 장·목록으로 가면 소리가 남지 않게
        unsafe { self.player.pause() };
    }
}
