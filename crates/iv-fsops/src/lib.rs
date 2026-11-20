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

/// 파일을 `dest` 폴더에 복사한다. 같은 이름이 있으면 덮어쓰지 않고 "이름 사본.jpg", "이름 사본 2.jpg" … 로.
/// 이름은 `create_new` 로 먼저 잡아 두므로 그 사이 다른 파일이 생겨도 덮어쓰지 않는다. 돌려주는 값 = 새 파일 경로.
pub fn copy_into(file: &Path, dest: &Path) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(file).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("폴더는 붙여넣을 수 없습니다".into());
    }
    if !dest.is_dir() {
        return Err("대상 폴더가 없습니다".into());
    }
    let name = file.file_name().ok_or("파일 이름 없음")?.to_string_lossy().into_owned();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    for k in 0..1000 {
        let candidate = match k {
            0 => name.clone(),
            1 => format!("{stem} 사본{ext}"),
            _ => format!("{stem} 사본 {k}{ext}"),
        };
        let to = dest.join(&candidate);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&to) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
        // 잡아 둔 빈 파일(우리 것)에 내용을 채운다
        let copied = std::fs::copy(file, &to);
        if !matches!(copied, Ok(n) if n == meta.len()) {
            let _ = std::fs::remove_file(&to);
            return Err(copied.err().map_or("복사 크기가 맞지 않음".into(), |e| e.to_string()));
        }
        return Ok(to);
    }
    Err("빈 이름을 찾지 못함".into())
}

//#region 폴더 휴지통 안전 판정
/// 시스템 폴더 (이 폴더와 그 하위는 지울 수 없다). 소문자 비교.
const SYSTEM_DIRS: &[&str] = &["/system", "/library", "/applications", "/usr", "/bin", "/sbin", "/private", "/etc", "/var", "/opt", "/cores", "/dev"];

/// 번들 확장자 (iv-fstree 의 판정과 같은 목록 — 역방향 의존을 피하려 복제)
fn is_bundle_name(name: &str) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "app" | "bundle" | "framework" | "photoslibrary" | "pkg"))
}

/// `a` 가 `b` 와 같거나 `b` 의 하위인가 (대소문자 무시 볼륨을 고려해 소문자로도 비교)
fn is_within(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || PathBuf::from(a.to_string_lossy().to_lowercase()).starts_with(b.to_string_lossy().to_lowercase())
}

/// 폴더를 휴지통에 넣으면 안 되는 이유(사용자에게 보이는 짧은 문장). 넣어도 되면 `None`.
/// `roots` = 앱의 트리 루트들(조상·자신 모두 금지), `protected` = 앱이 쓰는 폴더(캐시·휴지통 등, 조상·자신 금지).
/// 경로는 원래 값과 `canonicalize` 한 값 둘 다로 검사한다.
pub fn folder_trash_block(path: &Path, roots: &[PathBuf], protected: &[PathBuf]) -> Option<&'static str> {
    use std::os::unix::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return Some("폴더가 없습니다"),
    };
    if meta.file_type().is_symlink() {
        return Some("링크는 지울 수 없습니다");
    }
    if !meta.is_dir() {
        return Some("폴더가 아닙니다");
    }
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.components().count() >= 2);
    let home_canon = home.as_ref().and_then(|h| std::fs::canonicalize(h).ok());

    for p in [path, canon.as_path()] {
        if p.components().count() < 3 {
            return Some("이 폴더는 너무 상위라 지울 수 없습니다");
        }
        // 점으로 시작하는 폴더·번들
        for c in p.components() {
            if let std::path::Component::Normal(n) = c {
                let n = n.to_string_lossy();
                if n.starts_with('.') {
                    return Some("숨김 폴더는 지울 수 없습니다");
                }
                if is_bundle_name(&n) {
                    return Some("앱·번들 폴더는 지울 수 없습니다");
                }
            }
        }
        // Finder 숨김 플래그(UF_HIDDEN) — 대상이나 그 조상 (트리·스캔이 숨기는 것과 같은 규칙).
        // /Volumes 같은 시스템 상위는 원래 숨김이라 보지 않는다: 디스크·홈(/X/Y) 아래, 트리 루트 아래만.
        if p.ancestors().take_while(|a| a.components().count() > 3 && !roots.iter().any(|r| r == a)).any(finder_hidden) {
            return Some("숨김 폴더는 지울 수 없습니다");
        }
        // 볼륨 루트 /Volumes/<이름>
        let lower = PathBuf::from(p.to_string_lossy().to_lowercase());
        if lower.starts_with("/volumes") && lower.components().count() <= 3 {
            return Some("볼륨 전체는 지울 수 없습니다");
        }
        for r in roots {
            let rc = std::fs::canonicalize(r).ok();
            if is_within(r, p) || rc.as_ref().is_some_and(|rc| is_within(rc, p)) {
                return Some("열어 둔 최상위 폴더(또는 그 상위)는 지울 수 없습니다");
            }
        }
        for h in home.iter().chain(home_canon.iter()) {
            if is_within(h, p) {
                return Some("홈 폴더(또는 그 상위)는 지울 수 없습니다");
            }
            if is_within(p, &h.join("Library")) {
                return Some("라이브러리 폴더는 지울 수 없습니다");
            }
        }
        for s in SYSTEM_DIRS {
            if is_within(p, Path::new(s)) {
                return Some("시스템 폴더는 지울 수 없습니다");
            }
        }
        for q in protected {
            let qc = std::fs::canonicalize(q).ok();
            if is_within(q, p) || qc.as_ref().is_some_and(|qc| is_within(qc, p)) {
                return Some("앱이 쓰는 폴더(캐시·휴지통)는 지울 수 없습니다");
            }
        }
    }
    // 마운트 지점: 부모와 장치 번호가 다르다
    if let Some(parent) = canon.parent() {
        if let (Ok(a), Ok(b)) = (std::fs::metadata(&canon), std::fs::metadata(parent)) {
            if a.dev() != b.dev() {
                return Some("다른 저장 장치의 최상위 폴더는 지울 수 없습니다");
            }
        }
    }
    None
}
//#endregion

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iv-fsops-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 시험 폴더는 시스템 폴더(/private/var) 밖이어야 한다 — 저장소의 scratch/ (git 제외) 아래에 만든다
    fn safe_tmp(tag: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scratch").join(format!("fsops-trash-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    #[test]
    fn folder_trash_block_rules() {
        let d = safe_tmp("rules");
        let root = d.join("root");
        let sub = root.join("a/b");
        let outside = d.join("outside/x");
        let prot = d.join("cache");
        for p in [&sub, &outside, &prot.join("thumbs"), &d.join("root/.hid/y"), &d.join("root/X.app/c")] {
            std::fs::create_dir_all(p).unwrap();
        }
        let roots = vec![root.clone()];
        let prots = vec![prot.join("thumbs")];
        // 루트 하위는 허용
        assert_eq!(folder_trash_block(&sub, &roots, &prots), None);
        // Finder 숨김 플래그 폴더와 그 안은 금지
        let fh = root.join("finderhid/in");
        std::fs::create_dir_all(&fh).unwrap();
        assert_eq!(folder_trash_block(&fh, &roots, &prots), None);
        assert!(std::process::Command::new("chflags").arg("hidden").arg(root.join("finderhid")).status().unwrap().success());
        assert!(folder_trash_block(&root.join("finderhid"), &roots, &prots).is_some());
        assert!(folder_trash_block(&fh, &roots, &prots).is_some());
        // 루트 자신·조상 금지
        assert!(folder_trash_block(&root, &roots, &prots).is_some());
        assert!(folder_trash_block(&d, &roots, &prots).is_some());
        // protected 자신·조상 금지
        assert!(folder_trash_block(&prot.join("thumbs"), &roots, &prots).is_some());
        assert!(folder_trash_block(&prot, &roots, &prots).is_some());
        // 숨김·번들
        assert!(folder_trash_block(&d.join("root/.hid/y"), &[], &[]).is_some());
        assert!(folder_trash_block(&d.join("root/X.app/c"), &[], &[]).is_some());
        // 링크·없는 경로·파일
        std::os::unix::fs::symlink(&sub, d.join("link")).unwrap();
        assert!(folder_trash_block(&d.join("link"), &[], &[]).is_some());
        assert!(folder_trash_block(&d.join("nope"), &[], &[]).is_some());
        std::fs::write(d.join("f.txt"), b"x").unwrap();
        assert!(folder_trash_block(&d.join("f.txt"), &[], &[]).is_some());
        // 대소문자만 다른 루트 (대소문자 무시 볼륨 가정)
        let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
        assert!(folder_trash_block(&root, &[upper], &[]).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn folder_trash_block_constant_paths() {
        for p in ["/", "/Users", "/System/Library", "/Library", "/Applications", "/usr/local", "/private/tmp", "/Volumes", "/tmp", "/var/folders"] {
            assert!(folder_trash_block(Path::new(p), &[], &[]).is_some(), "{p} 는 막혀야 한다");
        }
        if let Some(h) = std::env::var_os("HOME") {
            let h = PathBuf::from(h);
            assert!(folder_trash_block(&h, &[], &[]).is_some());
            assert!(folder_trash_block(&h.join("Library"), &[], &[]).is_some());
            assert!(folder_trash_block(&h.join("Library/Caches"), &[], &[]).is_some());
        }
        for v in std::fs::read_dir("/Volumes").into_iter().flatten().flatten() {
            assert!(folder_trash_block(&v.path(), &[], &[]).is_some(), "{:?}", v.path());
        }
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

    #[test]
    fn copy_into_never_overwrites() {
        let d = tmp("copy");
        let src = d.join("a b.jpg");
        std::fs::write(&src, b"12345").unwrap();
        let sub = d.join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert_eq!(copy_into(&src, &sub).unwrap(), sub.join("a b.jpg"));
        // 같은 폴더에 붙이면 사본, 또 붙이면 사본 2
        assert_eq!(copy_into(&src, &d).unwrap(), d.join("a b 사본.jpg"));
        assert_eq!(copy_into(&src, &d).unwrap(), d.join("a b 사본 2.jpg"));
        assert_eq!(std::fs::read(d.join("a b 사본 2.jpg")).unwrap(), b"12345");
        assert_eq!(std::fs::read(&src).unwrap(), b"12345");
        assert!(copy_into(&sub, &d).is_err(), "폴더는 거부");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Finder 숨김 플래그(UF_HIDDEN, chflags hidden) — 링크는 따라가지 않는다
#[cfg(target_os = "macos")]
fn finder_hidden(p: &Path) -> bool {
    use std::os::macos::fs::MetadataExt;
    const UF_HIDDEN: u32 = 0x8000;
    std::fs::symlink_metadata(p).is_ok_and(|m| m.st_flags() & UF_HIDDEN != 0)
}

#[cfg(not(target_os = "macos"))]
fn finder_hidden(_p: &Path) -> bool {
    false
}
