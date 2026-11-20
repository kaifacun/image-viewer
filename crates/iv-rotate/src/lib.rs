//! 사진의 방향 태그만 바꿔 저장한다 (전체 보기에서 돌린 상태 그대로 저장).
//!
//! - 픽셀은 다시 압축하지 않는다 — JPEG 화질 손실 없음, 몇 번을 돌려 저장해도 같다.
//! - 원본을 바로 덮어쓰지 않는다: 같은 폴더의 점(.) 임시 파일에 쓰고, 다시 열어 방향·크기를 확인한 뒤
//!   확장 속성·권한을 옮기고 rename 으로 바꿔 넣는다. 어느 단계든 실패하면 원본은 그대로, 임시 파일은 지운다.
//! - 방향 태그를 쓸 수 없는 형식(RAW·여러 프레임·쓰기 미지원)은 오류로 돌려준다 — 몰래 재인코딩하지 않는다.

use std::path::Path;

use iv_core::Orientation;

/// `path` 의 방향 태그를 `target` 으로 바꿔 저장한다.
pub fn save_orientation(path: &Path, target: Orientation) -> Result<(), String> {
    imp::save_orientation(path, target)
}

/// 파일에 적힌 방향 (확인·시험용)
pub fn read_orientation(path: &Path) -> Option<Orientation> {
    imp::read_orientation(path)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use iv_core::Orientation;
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CFURL};
    use objc2_image_io::{
        CGImageDestination, CGImageSource, kCGImageDestinationOrientation, kCGImagePropertyOrientation,
        kCGImagePropertyPixelHeight, kCGImagePropertyPixelWidth, kCGImageSourceShouldCache,
    };

    //#region ImageIO 도우미
    fn open(path: &Path) -> Result<CFRetained<CGImageSource>, String> {
        let url = CFURL::from_file_path(path).ok_or("경로를 URL 로 바꾸지 못함")?;
        let no_cache = CFBoolean::new(false);
        let opts = CFDictionary::<CFString, CFType>::from_slices(&[unsafe { kCGImageSourceShouldCache }], &[no_cache.as_ref()]);
        unsafe { CGImageSource::with_url(&url, Some(opts.as_opaque())) }
            .filter(|s| unsafe { s.count() } > 0)
            .ok_or_else(|| "사진을 열 수 없음".to_string())
    }

    /// (크기, 방향, 프레임 수)
    fn info(src: &CGImageSource) -> Option<([i64; 2], Orientation, usize)> {
        let props = unsafe { src.properties_at_index(0, None) }?;
        // SAFETY: ImageIO 속성 사전은 CFString 키를 쓴다
        let props: CFRetained<CFDictionary<CFString, CFType>> = unsafe { CFRetained::cast_unchecked(props) };
        let num = |k: &CFString| props.get(k).and_then(|v| v.downcast::<CFNumber>().ok()).and_then(|n| n.as_i64());
        let w = num(unsafe { kCGImagePropertyPixelWidth })?;
        let h = num(unsafe { kCGImagePropertyPixelHeight })?;
        let o = num(unsafe { kCGImagePropertyOrientation }).unwrap_or(1);
        Some(([w, h], Orientation::from_exif(o as u32), unsafe { src.count() }))
    }

    pub fn read_orientation(path: &Path) -> Option<Orientation> {
        let src = open(path).ok()?;
        info(&src).map(|i| i.1)
    }
    //#endregion

    pub fn save_orientation(path: &Path, target: Orientation) -> Result<(), String> {
        let src = open(path)?;
        let (size, current, frames) = info(&src).ok_or("사진 정보를 읽지 못함")?;
        if current == target {
            return Ok(());
        }
        if frames != 1 {
            return Err("여러 장이 든 파일은 돌려 저장할 수 없음".into());
        }
        let uti = unsafe { src.r#type() }.ok_or("형식을 알 수 없음")?;
        let tmp = temp_path(path)?;
        let _ = std::fs::remove_file(&tmp); // 지난번에 남은 것
        // 폴더에 쓸 수 있는지 먼저 (ImageIO 는 이유 없이 실패만 알린다)
        std::fs::File::create(&tmp).map_err(|e| format!("폴더에 쓸 수 없음: {e}"))?;
        let result = write_temp(&src, &uti, &tmp, target)
            .and_then(|()| verify(&tmp, size, target))
            .and_then(|()| copy_metadata(path, &tmp))
            .and_then(|()| touch(&tmp))
            .and_then(|()| std::fs::rename(&tmp, path).map_err(|e| format!("바꿔 넣지 못함: {e}")));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// 수정 시각을 지금으로 — ImageIO 가 원본 시각을 옮겨 오고 JPEG 은 크기도 같아서,
    /// 그대로 두면 항목 id·썸네일 디스크 캐시(경로·mtime·크기)가 바뀐 줄 모른다.
    fn touch(p: &Path) -> Result<(), String> {
        std::fs::File::options()
            .write(true)
            .open(p)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()))
            .map_err(|e| format!("수정 시각을 못 바꿈: {e}"))
    }

    /// 같은 폴더의 점 파일 — 스캐너·감시·격자가 모두 무시한다 (확장자도 사진이 아니다)
    fn temp_path(path: &Path) -> Result<PathBuf, String> {
        let name = path.file_name().ok_or("파일 이름 없음")?.to_string_lossy();
        Ok(path.with_file_name(format!(".{name}.kaiiv-rotate")))
    }

    fn write_temp(src: &CGImageSource, uti: &CFString, tmp: &Path, target: Orientation) -> Result<(), String> {
        let url = CFURL::from_file_path(tmp).ok_or("임시 경로를 URL 로 바꾸지 못함")?;
        let dest = unsafe { CGImageDestination::with_url(&url, uti, 1, None) }
            .ok_or_else(|| format!("이 형식({uti})은 저장할 수 없음"))?;
        let n = CFNumber::new_i32(target.exif() as i32);
        let opts = CFDictionary::<CFString, CFType>::from_slices(&[unsafe { kCGImageDestinationOrientation }], &[n.as_ref()]);
        // 픽셀은 그대로, 메타데이터만 고쳐 복사 (성공하면 스스로 마무리한다 — finalize 를 부르지 않는다)
        let ok = unsafe { dest.copy_image_source(src, Some(opts.as_opaque()), std::ptr::null_mut()) };
        if ok { Ok(()) } else { Err(format!("이 형식({uti})은 방향만 바꿔 저장할 수 없음")) }
    }

    fn verify(tmp: &Path, size: [i64; 2], target: Orientation) -> Result<(), String> {
        let src = open(tmp).map_err(|e| format!("저장한 파일 확인 실패: {e}"))?;
        match info(&src) {
            Some((s, o, 1)) if s == size && o == target => Ok(()),
            _ => Err("저장한 파일 확인 실패 (방향·크기가 다름)".into()),
        }
    }

    /// 확장 속성(Finder 태그 등)과 권한을 원본에서 옮긴다
    fn copy_metadata(from: &Path, to: &Path) -> Result<(), String> {
        let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(|_| "경로에 NUL".to_string());
        let (f, t) = (c(from)?, c(to)?);
        let flags = libc::COPYFILE_XATTR | libc::COPYFILE_SECURITY;
        if unsafe { libc::copyfile(f.as_ptr(), t.as_ptr(), std::ptr::null_mut(), flags) } != 0 {
            return Err(format!("속성을 옮기지 못함: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;

    use iv_core::Orientation;

    pub fn save_orientation(_path: &Path, _target: Orientation) -> Result<(), String> {
        Err("이 OS 에서는 아직 돌려 저장할 수 없음".into())
    }

    pub fn read_orientation(_path: &Path) -> Option<Orientation> {
        None
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str, tag: &str) -> Option<PathBuf> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scratch/fixtures").join(name);
        if !src.exists() {
            eprintln!("시험 그림 없음 {} — tools/make-fixtures.py", src.display());
            return None;
        }
        let dir = std::env::temp_dir().join(format!("iv-rotate-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dst = dir.join(name);
        std::fs::copy(&src, &dst).unwrap();
        Some(dst)
    }

    /// 저장 방향 픽셀 (image-rs 는 EXIF 회전을 적용하지 않는다)
    fn pixels(p: &Path) -> Vec<u8> {
        image::open(p).unwrap().to_rgba8().into_raw()
    }

    /// 여러 형식에 방향 저장이 되는지 — 어떤 형식이 되는지 알려고 둔다 (실패해도 원본은 그대로여야 한다)
    #[test]
    fn probe_formats() {
        let Some(jpg) = fixture("p00.jpg", "probe") else { return };
        let dir = jpg.parent().unwrap().to_path_buf();
        let mut files = vec![jpg.clone()];
        for (fmt, ext) in [("png", "png"), ("tiff", "tif"), ("heic", "heic")] {
            let out = dir.join(format!("p00.{ext}"));
            let ok = std::process::Command::new("sips").args(["-s", "format", fmt]).arg(&jpg).arg("--out").arg(&out).output().map(|o| o.status.success()).unwrap_or(false);
            if ok {
                files.push(out);
            }
        }
        for f in files {
            let before = std::fs::read(&f).unwrap();
            let r = save_orientation(&f, Orientation::Rotate270);
            let now = read_orientation(&f);
            let after = std::fs::metadata(&f).unwrap().len();
            eprintln!("{}: {:?} → {:?} ({} → {} B)", f.display(), r, now, before.len(), after);
            match r {
                Ok(()) => assert_eq!(now, Some(Orientation::Rotate270)),
                Err(_) => assert_eq!(std::fs::read(&f).unwrap(), before, "실패하면 원본은 그대로"),
            }
        }
    }

    #[test]
    fn jpeg_orientation_round_trip_keeps_bytes_outside_metadata() {
        let Some(p) = fixture("orient1.jpg", "jpg") else { return };
        let before_len = std::fs::metadata(&p).unwrap().len();
        let px = pixels(&p);
        assert_eq!(read_orientation(&p), Some(Orientation::Normal));
        let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        save_orientation(&p, Orientation::Rotate90).unwrap();
        assert_eq!(read_orientation(&p), Some(Orientation::Rotate90));
        assert_ne!(std::fs::metadata(&p).unwrap().modified().unwrap(), mtime, "수정 시각이 바뀌어야 캐시가 새로 만든다");
        assert!(pixels(&p) == px, "픽셀이 다시 압축되면 안 된다");
        save_orientation(&p, Orientation::Normal).unwrap();
        assert_eq!(read_orientation(&p), Some(Orientation::Normal));
        // 임시 파일이 남지 않는다
        let left: Vec<_> = std::fs::read_dir(p.parent().unwrap()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).collect();
        assert!(left.is_empty());
        // 재압축이면 크기가 크게 달라진다 — 메타데이터만 바뀌면 거의 같다
        let after_len = std::fs::metadata(&p).unwrap().len();
        assert!((after_len as i64 - before_len as i64).abs() < 4096, "{before_len} → {after_len}");
    }

    #[test]
    fn unsupported_or_missing_leaves_original() {
        let missing = std::env::temp_dir().join("iv-rotate-none.jpg");
        assert!(save_orientation(&missing, Orientation::Rotate90).is_err());
    }
}
