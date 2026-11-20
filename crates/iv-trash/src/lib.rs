//! 파일을 OS 휴지통으로 보낸다. 영구 삭제(`remove_file`)로는 절대 대체하지 않는다 —
//! 휴지통이 없는 볼륨·권한 문제 등으로 실패하면 오류를 돌려주고 파일은 그대로 둔다.

use std::path::Path;

//#region 시험용 우회 (IV_TRASH_DIR)
/// `IV_TRASH_DIR` 이 있으면 OS 휴지통 대신 그 폴더로 옮긴다 (자가 시험이 사용자 휴지통을 더럽히지 않게).
fn test_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("IV_TRASH_DIR").map(Into::into)
}

fn move_to_dir(path: &Path, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = path.file_name().ok_or("파일 이름 없음")?;
    // 같은 이름이 있으면 "이름 2", "이름 3"… 으로 피한다 (덮어쓰기 금지)
    let mut dest = dir.join(name);
    let mut n = 2;
    while dest.symlink_metadata().is_ok() {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let new = match path.extension() {
            Some(ext) if path.is_file() => format!("{stem} {n}.{}", ext.to_string_lossy()),
            _ => format!("{} {n}", name.to_string_lossy()),
        };
        dest = dir.join(new);
        n += 1;
    }
    std::fs::rename(path, dest).map_err(|e| e.to_string())
}
//#endregion

/// 파일 하나를 휴지통으로. 실패하면 사람이 읽을 오류 문자열.
pub fn trash(path: &Path) -> Result<(), String> {
    if let Some(dir) = test_dir() {
        return move_to_dir(path, &dir);
    }
    imp::trash(path)
}

//#region macOS — NSFileManager trashItemAtURL (Finder 의 "휴지통으로 이동" 과 같음, 되돌리기 가능)
#[cfg(target_os = "macos")]
mod imp {
    use std::path::Path;

    use objc2_foundation::{NSFileManager, NSString, NSURL};

    pub fn trash(path: &Path) -> Result<(), String> {
        let s = path.to_str().ok_or("경로가 UTF-8 이 아님")?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(s));
        let fm = NSFileManager::defaultManager();
        fm.trashItemAtURL_resultingItemURL_error(&url, None).map_err(|e| e.localizedDescription().to_string())
    }
}
//#endregion

//#region 그 밖의 OS — 아직 미구현 (영구 삭제로 대체하지 않는다)
#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;

    pub fn trash(_path: &Path) -> Result<(), String> {
        Err("이 OS 에서는 아직 휴지통 이동을 지원하지 않습니다".into())
    }
}
//#endregion

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_to_dir_avoids_name_collisions() {
        let base = std::env::temp_dir().join(format!("iv-trash-coll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("bin");
        for i in 0..3 {
            let src = base.join(format!("src{i}"));
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("a.jpg"), format!("{i}")).unwrap();
            std::fs::create_dir_all(src.join("folder")).unwrap();
            std::fs::write(src.join("folder/in.txt"), format!("{i}")).unwrap();
            move_to_dir(&src.join("a.jpg"), &dir).unwrap();
            move_to_dir(&src.join("folder"), &dir).unwrap();
        }
        assert_eq!(std::fs::read_to_string(dir.join("a.jpg")).unwrap(), "0");
        assert_eq!(std::fs::read_to_string(dir.join("a 2.jpg")).unwrap(), "1");
        assert_eq!(std::fs::read_to_string(dir.join("a 3.jpg")).unwrap(), "2");
        assert_eq!(std::fs::read_to_string(dir.join("folder 3/in.txt")).unwrap(), "2");
        assert!(dir.join("folder 2").is_dir() && dir.join("folder").is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_file_errors_and_never_panics() {
        let p = std::env::temp_dir().join("iv-trash-definitely-missing-file.jpg");
        assert!(imp::trash(&p).is_err());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod real_trash {
    /// 실제 OS 휴지통 호출 확인 (사용자 휴지통에 임시 파일 하나가 들어간다 — 수동 실행만)
    #[test]
    #[ignore]
    fn moves_temp_file_to_os_trash() {
        let p = std::env::temp_dir().join("kaiiv-trash-selftest.txt");
        std::fs::write(&p, b"KaiIV trash self-test").unwrap();
        super::imp::trash(&p).unwrap();
        assert!(!p.exists());
        let home = std::env::var_os("HOME").unwrap();
        let in_trash = std::fs::read_dir(std::path::Path::new(&home).join(".Trash")).map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("kaiiv-trash-selftest"))).unwrap_or(false);
        eprintln!("~/.Trash 에서 보임: {in_trash} (Full Disk Access 없으면 목록을 못 읽어 false 일 수 있음)");
    }
}
