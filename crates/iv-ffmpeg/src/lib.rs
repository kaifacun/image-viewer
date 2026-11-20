//! FFmpeg(별도 프로그램)로 macOS 기본 엔진이 못 여는 영상을 다룬다.
//!
//! - [`probe`]: 코덱·길이·화면 크기 (ffprobe)
//! - [`poster_png`]: 대표 장면 한 장 (PNG 바이트 — 썸네일·미리보기)
//! - [`Job`]: 재생용 MOV 로 바꾸기. 코덱이 맞으면 **옮겨 담기만**(재압축 없음, 몇 초), 아니면 맥 하드웨어
//!   인코더(HEVC VideoToolbox)로 재인코딩. 결과는 캐시에 남겨 다음엔 바로 연다.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use iv_codecpack::Tools;

/// 영상 정보
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub video: Option<String>,
    pub audio: Option<String>,
    pub duration: f64,
    /// 화면에 보이는 크기 (회전 반영)
    pub size: [u32; 2],
}

/// macOS 기본 엔진(AVFoundation)이 MOV 안에서 재생하는 코덱 — 이것이면 옮겨 담기만
const NATIVE_VIDEO: &[&str] = &["h264", "hevc", "prores", "mjpeg"];
const NATIVE_AUDIO: &[&str] = &["aac", "alac", "mp3", "ac3", "eac3", "pcm_s16le", "pcm_s24le", "pcm_s32le", "pcm_f32le", "pcm_s16be", "pcm_s24be"];

impl Probe {
    pub fn video_native(&self) -> bool {
        self.video.as_deref().is_some_and(|v| NATIVE_VIDEO.contains(&v))
    }

    pub fn audio_native(&self) -> bool {
        self.audio.as_deref().is_none_or(|a| NATIVE_AUDIO.contains(&a))
    }

    /// 다시 압축 없이 옮겨 담기만 하면 되는가
    pub fn remux_only(&self) -> bool {
        self.video_native() && self.audio_native()
    }
}

pub fn probe(t: &Tools, p: &Path) -> Result<Probe, String> {
    let out = Command::new(&t.ffprobe)
        .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,width,height:stream_side_data=rotation:format=duration", "-of", "default=nw=1"])
        .arg(p)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ffprobe 실행 실패: {e}"))?;
    if !out.status.success() {
        return Err(format!("영상을 읽지 못함: {}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("")));
    }
    let mut pr = Probe::default();
    // 줄 순서: 스트림마다 codec_name → codec_type → width/height → (rotation)
    let (mut name, mut kind) = (None::<String>, None::<String>);
    let (mut w, mut h, mut rot) = (0u32, 0u32, 0i32);
    let mut in_video = false;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "codec_name" => name = Some(v.to_string()),
            "codec_type" => {
                kind = Some(v.to_string());
                in_video = v == "video" && pr.video.is_none();
                match v {
                    "video" if pr.video.is_none() => pr.video = name.take(),
                    "audio" if pr.audio.is_none() => pr.audio = name.take(),
                    _ => {}
                }
            }
            "width" if in_video => w = v.parse().unwrap_or(0),
            "height" if in_video => h = v.parse().unwrap_or(0),
            "rotation" if in_video => rot = v.parse::<f64>().unwrap_or(0.0) as i32,
            "duration" => pr.duration = v.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    let _ = kind;
    if pr.video.is_none() {
        return Err("영상 트랙이 없음".into());
    }
    pr.size = if rot.rem_euclid(180) == 90 { [h, w] } else { [w, h] };
    Ok(pr)
}

/// 대표 장면 (길이의 10%, 최대 1초 지점) — 긴 변 `max_side` 이하의 PNG
pub fn poster_png(t: &Tools, p: &Path, duration: f64, max_side: u32) -> Result<Vec<u8>, String> {
    let at = if duration > 0.0 { (duration * 0.1).min(1.0) } else { 0.0 };
    let scale = format!("scale='min({m},iw)':'min({m},ih)':force_original_aspect_ratio=decrease", m = max_side.max(16));
    let out = Command::new(&t.ffmpeg)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-ss", &format!("{at:.3}"), "-i"])
        .arg(p)
        .args(["-frames:v", "1", "-vf", &scale, "-f", "image2pipe", "-vcodec", "png", "pipe:1"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ffmpeg 실행 실패: {e}"))?;
    if out.stdout.is_empty() {
        return Err(format!("대표 장면 실패: {}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("")));
    }
    Ok(out.stdout)
}

//#region 재생용 변환 — 캐시 경로, 백그라운드 작업
/// 변환 결과 캐시 파일 (원본 경로·수정 시각·크기가 같으면 같은 파일)
pub fn cache_path(cache_dir: &Path, src: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(src).ok();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    meta.as_ref().and_then(|m| m.modified().ok()).hash(&mut h);
    meta.map(|m| m.len()).hash(&mut h);
    cache_dir.join(format!("{:016x}.mov", h.finish()))
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    /// 진행률 0~1, 옮겨 담기만 하는가
    Running(f32, bool),
    Done(PathBuf),
    Failed(String),
}

pub struct Job {
    state: Arc<Mutex<JobState>>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    /// `src` 를 `out` 으로 바꾸기 시작. 끝나거나 진행이 있을 때 `wake`.
    pub fn start(t: Tools, src: PathBuf, out: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let state = Arc::new(Mutex::new(JobState::Running(0.0, false)));
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c) = (state.clone(), cancel.clone());
        std::thread::Builder::new()
            .name("iv-ffmpeg-convert".into())
            .spawn(move || {
                let set = |st: JobState| {
                    *s.lock().unwrap_or_else(|e| e.into_inner()) = st;
                    wake();
                };
                let r = convert(&t, &src, &out, &c, &|p, remux| set(JobState::Running(p, remux)));
                set(match r {
                    Ok(()) => JobState::Done(out),
                    Err(e) => JobState::Failed(e),
                });
            })
            .ok();
        Self { state, cancel }
    }

    pub fn state(&self) -> JobState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Job {
    /// 다른 영상으로 가면 변환을 멈춘다
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn convert(t: &Tools, src: &Path, out: &Path, cancel: &AtomicBool, progress: &dyn Fn(f32, bool)) -> Result<(), String> {
    let pr = probe(t, src)?;
    let remux = pr.remux_only();
    progress(0.0, remux);
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    // 하드웨어 인코더 → 실패하면 소프트웨어(x264)로 한 번 더
    let encoders: &[&[&str]] = if pr.video_native() {
        &[&[]]
    } else {
        &[&["-c:v", "hevc_videotoolbox", "-q:v", "62", "-tag:v", "hvc1", "-allow_sw", "1"], &["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"]]
    };
    let mut last = String::new();
    for enc in encoders {
        let tmp = out.with_extension("part.mov");
        let mut cmd = Command::new(&t.ffmpeg);
        cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(src).args(["-map", "0:v:0", "-map", "0:a:0?", "-sn", "-dn"]);
        if pr.video_native() {
            cmd.args(["-c:v", "copy"]);
            if pr.video.as_deref() == Some("hevc") {
                cmd.args(["-tag:v", "hvc1"]);
            }
        } else {
            cmd.args(*enc);
        }
        if pr.audio_native() {
            cmd.args(["-c:a", "copy"]);
        } else {
            cmd.args(["-c:a", "aac", "-b:a", "192k"]);
        }
        cmd.args(["-movflags", "+faststart", "-progress", "pipe:1", "-nostats", "-f", "mov"]).arg(&tmp);
        let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("ffmpeg 실행 실패: {e}"))?;
        //#region 진행률 — out_time_us / 길이
        if let Some(so) = child.stdout.take() {
            for line in BufReader::new(so).lines().map_while(Result::ok) {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    break;
                }
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
                    if pr.duration > 0.0 {
                        progress(((us / 1e6) / pr.duration).clamp(0.0, 1.0) as f32, remux);
                    }
                }
            }
        }
        //#endregion
        let st = child.wait().map_err(|e| e.to_string())?;
        if cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&tmp);
            return Err("취소됨".into());
        }
        if st.success() && tmp.is_file() {
            std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
            log::info!("변환 완료 ({}): {}", if remux { "옮겨 담기" } else { "재인코딩" }, src.display());
            return Ok(());
        }
        let mut err = String::new();
        if let Some(mut se) = child.stderr.take() {
            use std::io::Read;
            let _ = se.read_to_string(&mut err);
        }
        last = err.lines().last().unwrap_or("변환 실패").to_string();
        let _ = std::fs::remove_file(&tmp);
        log::warn!("변환 실패, 다음 방법: {last}");
    }
    Err(format!("변환 실패: {last}"))
}
//#endregion

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Option<(Tools, PathBuf)> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scratch/videos").join(name);
        let t = iv_codecpack::find()?;
        p.exists().then_some((t, p))
    }

    #[test]
    fn probe_and_plans() {
        let Some((t, p)) = fixture("counter.mkv") else { return };
        let pr = probe(&t, &p).unwrap();
        assert_eq!(pr.video.as_deref(), Some("h264"));
        assert!(pr.remux_only());
        assert_eq!(pr.size, [1280, 720]);
        let Some((t, p)) = fixture("counter.webm") else { return };
        let pr = probe(&t, &p).unwrap();
        assert_eq!(pr.video.as_deref(), Some("vp9"));
        assert!(!pr.remux_only());
        let png = poster_png(&t, &p, pr.duration, 320).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
    }

    #[test]
    fn converts_remux_and_transcode() {
        for name in ["counter.mkv", "counter.webm"] {
            let Some((t, p)) = fixture(name) else { return };
            let out = std::env::temp_dir().join(format!("iv-ffmpeg-{}-{name}.mov", std::process::id()));
            let job = Job::start(t.clone(), p, out.clone(), || {});
            let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
            let st = loop {
                match job.state() {
                    JobState::Running(..) if std::time::Instant::now() < until => std::thread::sleep(std::time::Duration::from_millis(50)),
                    s => break s,
                }
            };
            assert_eq!(st, JobState::Done(out.clone()), "{name}");
            let pr = probe(&t, &out).unwrap();
            assert!(pr.video_native(), "{name} → {:?}", pr.video);
            let _ = std::fs::remove_file(out);
        }
    }
}
