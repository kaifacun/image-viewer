//! 파일·폴더 작업. **절대 덮어쓰지 않는다** — 같은 이름이 있으면 오류.
//! 오류는 사람이 읽을 한국어 문자열.

use std::path::{Path, PathBuf};

//#region 이름 검사
/// 새 이름이 쓸 수 있는지 (빈 이름·경로 구분자·점으로 시작(숨김이 되어 사라짐)·`.`/`..` 금지)
pub fn validate_name(name: &str) -> Result<&str, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("이름이 비어 있습니다".into());
    }
    if n.contains('/') || n.contains('\0') || n.contains(':') {
        return Err("이름에 / 나 : 는 쓸 수 없습니다".into());
    }
    if n.starts_with('.') {
        return Err("점(.)으로 시작하는 이름은 숨김이 되어 보이지 않습니다".into());
    }
    if n.len() > 255 {
        return Err("이름이 너무 깁니다".into());
    }
    Ok(n)
}

/// 같은 파일인가 (대소문자만 바꾸는 이름 바꾸기 — 대소문자 무시 볼륨에선 새 이름이 "이미 있음"으로 보인다)
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}
//#endregion

/// `parent` 안에 새 폴더
pub fn create_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let name = validate_name(name)?;
    let p = parent.join(name);
    if p.symlink_metadata().is_ok() {
        return Err(format!("'{name}' 이(가) 이미 있습니다"));
    }
    std::fs::create_dir(&p).map_err(|e| e.to_string())?;
    Ok(p)
}

/// 같은 폴더 안에서 이름만 바꾼다 (파일·폴더 모두)
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let name = validate_name(new_name)?;
    let parent = path.parent().ok_or("상위 폴더 없음")?;
    let to = parent.join(name);
    if to == path {
        return Ok(to);
    }
    if to.symlink_metadata().is_ok() && !same_file(path, &to) {
        return Err(format!("'{name}' 이(가) 이미 있습니다"));
    }
    std::fs::rename(path, &to).map_err(|e| e.to_string())?;
    Ok(to)
}

/// 파일을 `dest` 폴더로 옮긴다. 같은 볼륨이면 이름 바꾸기, 다른 볼륨이면 복사 → 크기 확인 → 원본 삭제.
pub fn move_into(file: &Path, dest: &Path) -> Result<PathBuf, String> {
    let name = file.file_name().ok_or("파일 이름 없음")?;
    if !dest.is_dir() {
        return Err("대상 폴더가 없습니다".into());
    }
    let to = dest.join(name);
    if file.parent() == Some(dest) {
        return Ok(to);
    }
    if to.symlink_metadata().is_ok() {
        return Err(format!("대상에 '{}' 이(가) 이미 있습니다", name.to_string_lossy()));
    }
    match std::fs::rename(file, &to) {
        Ok(()) => Ok(to),
        // 다른 볼륨 (EXDEV)
        Err(e) if e.raw_os_error() == Some(18) => {
            let len = std::fs::metadata(file).map_err(|e| e.to_string())?.len();
            let copied = std::fs::copy(file, &to).map_err(|e| e.to_string())?;
            if copied != len || std::fs::metadata(&to).map(|m| m.len()).ok() != Some(len) {
                let _ = std::fs::remove_file(&to);
                return Err("복사 크기가 맞지 않아 옮기지 않았습니다".into());
            }
            std::fs::remove_file(file).map_err(|e| format!("복사는 됐지만 원본을 지우지 못함: {e}"))?;
            Ok(to)
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iv-fsops-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names() {
        assert!(validate_name("  ").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name(".hidden").is_err());
        assert_eq!(validate_name("  여행 2026 ").unwrap(), "여행 2026");
    }

    #[test]
    fn create_rename_move_never_overwrite() {
        let d = tmp("ops");
        let f = create_folder(&d, "새 폴더").unwrap();
        assert!(f.is_dir());
        assert!(create_folder(&d, "새 폴더").is_err());
        std::fs::write(d.join("a.jpg"), b"aaa").unwrap();
        std::fs::write(d.join("b.jpg"), b"bbb").unwrap();
        // 덮어쓰기 금지
        assert!(rename(&d.join("a.jpg"), "b.jpg").is_err());
        assert_eq!(std::fs::read(d.join("b.jpg")).unwrap(), b"bbb");
        // 대소문자만 바꾸기
        let up = rename(&d.join("a.jpg"), "A.jpg").unwrap();
        assert!(up.ends_with("A.jpg") && up.exists());
        // 폴더 이름 바꾸기
        let g = rename(&f, "여행").unwrap();
        assert!(g.is_dir() && !f.exists());
        // 옮기기, 같은 이름이 있으면 거부
        let moved = move_into(&d.join("b.jpg"), &g).unwrap();
        assert_eq!(std::fs::read(&moved).unwrap(), b"bbb");
        std::fs::write(d.join("b.jpg"), b"new").unwrap();
        assert!(move_into(&d.join("b.jpg"), &g).is_err());
        assert_eq!(std::fs::read(&moved).unwrap(), b"bbb");
        let _ = std::fs::remove_dir_all(&d);
    }
}
