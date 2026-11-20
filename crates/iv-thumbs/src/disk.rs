//! 썸네일 디스크 캐시.
//!
//! 파일 = 헤더(매직·크기·원본 크기·원본 경로·mtime·len) + JPEG(불투명) 또는 PNG(알파).
//! 파일 이름은 128비트 해시지만, 읽을 때 헤더의 경로·mtime·len 을 대조하므로 해시 충돌이 나도 남의 썸네일을 보이지 않는다.
//! 쓰기는 임시 파일 → rename (원자적). 읽기 실패·손상은 조용히 무시하고 재생성한다.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use iv_core::{Frame, ImageSource};

use crate::{Thumb, ThumbRequest};

const MAGIC: &[u8; 4] = b"IVT1";
const VERSION_DIR: &str = "thumbs-v1";

/// 플랫폼 기본 캐시 위치. `IV_CACHE_DIR` 가 있으면 그것.
pub fn default_cache_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("IV_CACHE_DIR") {
        return Some(PathBuf::from(d));
    }
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/KaiIV"))
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|h| PathBuf::from(h).join("KaiIV").join("cache"))
    } else {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))).map(|p| p.join("kaiiv"))
    }
}

pub struct DiskCache {
    dir: PathBuf,
}

fn fnv(seed: u64, parts: &[&[u8]]) -> u64 {
    let mut h = seed;
    for p in parts {
        for b in *p {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn mtime_ns(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

fn path_of(src: &ImageSource) -> &Path {
    match src {
        ImageSource::Path(p) => p,
    }
}

impl DiskCache {
    pub fn open(root: PathBuf, side: u32) -> std::io::Result<Self> {
        let dir = root.join(format!("{VERSION_DIR}-{side}"));
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, req: &ThumbRequest) -> PathBuf {
        let p = path_of(&req.src).as_os_str().as_encoded_bytes();
        let m = mtime_ns(req.mtime).to_le_bytes();
        let l = req.len.to_le_bytes();
        let a = fnv(0xcbf29ce484222325, &[p, &m, &l]);
        let b = fnv(0x84222325cbf29ce4, &[&l, &m, p]);
        let name = format!("{a:016x}{b:016x}");
        self.dir.join(&name[..2]).join(format!("{}.ivt", &name[2..]))
    }

    pub fn load(&self, req: &ThumbRequest) -> Option<Thumb> {
        let bytes = std::fs::read(self.file(req)).ok()?;
        let r = decode_file(&bytes, req);
        if r.is_none() {
            log::debug!("썸네일 캐시 손상·불일치 무시: {}", req.src.display_name());
        }
        r
    }

    pub fn store(&self, req: &ThumbRequest, th: &Thumb) {
        let f = self.file(req);
        let Some(bytes) = encode_file(req, th) else { return };
        let res = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(f.parent().unwrap())?;
            let tmp = f.with_extension(format!("tmp{}-{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &f).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
        })();
        if let Err(e) = res {
            log::debug!("썸네일 캐시 쓰기 실패: {e}");
        }
    }

    /// 상한을 넘으면 오래된(mtime) 것부터 80% 까지 지운다. 시작 시 한 번, 백그라운드.
    pub fn prune_in_background(&self, limit: u64) {
        let dir = self.dir.clone();
        let started = SystemTime::now();
        let _ = std::thread::Builder::new().name("iv-thumb-prune".into()).spawn(move || {
            let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
            let Ok(shards) = std::fs::read_dir(&dir) else { return };
            for s in shards.flatten() {
                let Ok(rd) = std::fs::read_dir(s.path()) else { continue };
                for e in rd.flatten() {
                    if let Ok(m) = e.metadata() {
                        let t = m.modified().unwrap_or(UNIX_EPOCH);
                        files.push((t, m.len(), e.path()));
                    }
                }
            }
            let total: u64 = files.iter().map(|f| f.1).sum();
            if total <= limit {
                return;
            }
            files.sort();
            let mut cur = total;
            let target = limit / 10 * 8;
            for (t, len, p) in files {
                if cur <= target || t >= started {
                    break;
                }
                if std::fs::remove_file(&p).is_ok() {
                    cur -= len;
                }
            }
            log::info!("썸네일 캐시 정리: {} MB → {} MB", total >> 20, cur >> 20);
        });
    }
}

fn encode_file(req: &ThumbRequest, th: &Thumb) -> Option<Vec<u8>> {
    let img = &th.image;
    let opaque = img.pixels.chunks_exact(4).all(|p| p[3] == 255);
    let mut enc = Vec::new();
    if opaque {
        let rgb: Vec<u8> = img.pixels.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        let mut e = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut enc, 85);
        e.encode(&rgb, img.width, img.height, image::ExtendedColorType::Rgb8).ok()?;
    } else {
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut enc).write_image(&img.pixels, img.width, img.height, image::ExtendedColorType::Rgba8).ok()?;
    }
    let path = path_of(&req.src).as_os_str().as_encoded_bytes();
    let mut out = Vec::with_capacity(enc.len() + 64 + path.len());
    out.extend_from_slice(MAGIC);
    for v in [img.width, img.height, th.full_size[0], th.full_size[1], path.len() as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(path);
    out.extend_from_slice(&mtime_ns(req.mtime).to_le_bytes());
    out.extend_from_slice(&req.len.to_le_bytes());
    out.push(u8::from(!opaque));
    out.extend_from_slice(&enc);
    Some(out)
}

fn decode_file(bytes: &[u8], req: &ThumbRequest) -> Option<Thumb> {
    let mut c = Cursor::new(bytes);
    let mut magic = [0u8; 4];
    c.read_exact(&mut magic).ok()?;
    if &magic != MAGIC {
        return None;
    }
    let mut u32s = [0u32; 5];
    for v in &mut u32s {
        let mut b = [0u8; 4];
        c.read_exact(&mut b).ok()?;
        *v = u32::from_le_bytes(b);
    }
    let [w, h, fw, fh, plen] = u32s;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || plen > 64 * 1024 {
        return None;
    }
    let mut path = vec![0u8; plen as usize];
    c.read_exact(&mut path).ok()?;
    let mut b8 = [0u8; 8];
    c.read_exact(&mut b8).ok()?;
    let mtime = u64::from_le_bytes(b8);
    c.read_exact(&mut b8).ok()?;
    let len = u64::from_le_bytes(b8);
    // 지문 대조 — 해시 충돌·파일 변경 시 쓰지 않는다
    if path != path_of(&req.src).as_os_str().as_encoded_bytes() || mtime != mtime_ns(req.mtime) || len != req.len {
        return None;
    }
    let mut kind = [0u8; 1];
    c.read_exact(&mut kind).ok()?;
    let rest = &bytes[c.position() as usize..];
    let fmt = if kind[0] == 0 { image::ImageFormat::Jpeg } else { image::ImageFormat::Png };
    let img = image::load_from_memory_with_format(rest, fmt).ok()?.into_rgba8();
    if img.width() != w || img.height() != h {
        return None;
    }
    let frame = Frame::rgba8(w, h, img.into_raw(), [fw, fh]);
    Some(Thumb { key: req.key, image: Arc::new(frame), full_size: [fw, fh] })
}
