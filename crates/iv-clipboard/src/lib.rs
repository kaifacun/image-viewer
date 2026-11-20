//! 시스템 클립보드. 파일은 Finder 와 같은 형식(파일 URL 항목)으로 써서 Finder 에 붙여넣을 수 있고,
//! Finder 에서 복사한 파일도 읽는다. 각 항목에는 절대 경로 글도 함께 넣어 글 입력 칸에 붙이면 경로가 들어간다.
//!
//! `IV_CLIPBOARD_FILE` 이 있으면 시스템 클립보드 대신 그 파일을 쓴다 (자가 시험이 사용자 클립보드를 덮지 않게).

use std::path::{Path, PathBuf};

/// 파일들을 클립보드에. 돌려주는 값 = 쓴 뒤의 변경 번호 (잘라내기가 아직 유효한지 확인용)
pub fn write_files(paths: &[PathBuf]) -> Result<i64, String> {
    if let Some(f) = test_file() {
        let body: String = paths.iter().map(|p| format!("F\t{}\n", p.display())).collect();
        return test_write(&f, &body);
    }
    imp::write_files(paths)
}

/// 글(경로 등)을 클립보드에
pub fn write_text(text: &str) -> Result<i64, String> {
    if let Some(f) = test_file() {
        return test_write(&f, &format!("T\t{text}\n"));
    }
    imp::write_text(text)
}

/// 클립보드의 파일들 (없으면 빈 목록)
pub fn read_files() -> Vec<PathBuf> {
    if let Some(f) = test_file() {
        let s = std::fs::read_to_string(f).unwrap_or_default();
        return s.lines().skip(1).filter_map(|l| l.strip_prefix("F\t")).map(PathBuf::from).collect();
    }
    imp::read_files()
}

/// 클립보드 변경 번호 (다른 곳에서 복사하면 바뀐다)
pub fn change_count() -> i64 {
    if let Some(f) = test_file() {
        return std::fs::read_to_string(f).ok().and_then(|s| s.lines().next()?.parse().ok()).unwrap_or(0);
    }
    imp::change_count()
}

/// 클립보드의 글 (시험 확인용)
pub fn read_text() -> Option<String> {
    if let Some(f) = test_file() {
        let s = std::fs::read_to_string(f).ok()?;
        return s.lines().skip(1).find_map(|l| l.strip_prefix("T\t").map(str::to_string));
    }
    imp::read_text()
}

//#region 시험용 우회 (IV_CLIPBOARD_FILE) — 첫 줄 변경 번호, 다음 줄부터 F\t경로 / T\t글
fn test_file() -> Option<PathBuf> {
    std::env::var_os("IV_CLIPBOARD_FILE").map(Into::into)
}

fn test_write(f: &Path, body: &str) -> Result<i64, String> {
    let n = change_count() + 1;
    std::fs::write(f, format!("{n}\n{body}")).map_err(|e| e.to_string())?;
    Ok(n)
}
//#endregion

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;

    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeString, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};

    pub fn write_files(paths: &[PathBuf]) -> Result<i64, String> {
        let pb = NSPasteboard::generalPasteboard();
        let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
        for p in paths {
            let s = p.to_str().ok_or("경로가 UTF-8 이 아님")?;
            let url = NSURL::fileURLWithPath(&NSString::from_str(s));
            let url_s = url.absoluteString().ok_or("URL 변환 실패")?;
            let item = NSPasteboardItem::new();
            // 파일 URL (Finder 붙여넣기) + 절대 경로 글 (글 입력 칸 붙여넣기)
            item.setString_forType(&url_s, unsafe { NSPasteboardTypeFileURL });
            item.setString_forType(&NSString::from_str(s), unsafe { NSPasteboardTypeString });
            items.push(ProtocolObject::from_retained(item));
        }
        pb.clearContents();
        if !pb.writeObjects(&NSArray::from_retained_slice(&items)) {
            return Err("클립보드에 쓰지 못함".into());
        }
        Ok(pb.changeCount() as i64)
    }

    pub fn write_text(text: &str) -> Result<i64, String> {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        if !pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString }) {
            return Err("클립보드에 쓰지 못함".into());
        }
        Ok(pb.changeCount() as i64)
    }

    pub fn read_files() -> Vec<PathBuf> {
        let pb = NSPasteboard::generalPasteboard();
        let Some(items) = pb.pasteboardItems() else { return Vec::new() };
        items
            .to_vec()
            .into_iter()
            .filter_map(|it| it.stringForType(unsafe { NSPasteboardTypeFileURL }))
            .filter_map(|s| NSURL::URLWithString(&s))
            .filter(|u| u.isFileURL())
            .filter_map(|u| u.path())
            .map(|p| PathBuf::from(p.to_string()))
            .collect()
    }

    pub fn read_text() -> Option<String> {
        NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeString }).map(|s| s.to_string())
    }

    pub fn change_count() -> i64 {
        NSPasteboard::generalPasteboard().changeCount() as i64
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::PathBuf;

    pub fn write_files(_paths: &[PathBuf]) -> Result<i64, String> {
        Err("이 OS 에서는 아직 파일 복사를 지원하지 않음".into())
    }
    pub fn write_text(_text: &str) -> Result<i64, String> {
        Err("이 OS 에서는 아직 클립보드를 지원하지 않음".into())
    }
    pub fn read_files() -> Vec<PathBuf> {
        Vec::new()
    }
    pub fn read_text() -> Option<String> {
        None
    }
    pub fn change_count() -> i64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 시험용 파일 클립보드 왕복 (실제 클립보드는 건드리지 않는다)
    #[test]
    fn test_file_round_trip() {
        let f = std::env::temp_dir().join(format!("iv-clip-{}", std::process::id()));
        // SAFETY: 이 시험만 이 변수를 쓴다
        unsafe { std::env::set_var("IV_CLIPBOARD_FILE", &f) };
        let a = write_files(&[PathBuf::from("/a/b.jpg"), PathBuf::from("/c d/e.png")]).unwrap();
        assert_eq!(read_files(), vec![PathBuf::from("/a/b.jpg"), PathBuf::from("/c d/e.png")]);
        let b = write_text("/x/y.jpg").unwrap();
        assert!(b > a);
        assert_eq!(change_count(), b);
        assert!(read_files().is_empty());
        assert_eq!(read_text().as_deref(), Some("/x/y.jpg"));
        let _ = std::fs::remove_file(f);
    }
}
