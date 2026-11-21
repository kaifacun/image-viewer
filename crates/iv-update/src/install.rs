//! 받기 · 풀어 검사 · 같은 볼륨 교체 · 다시 실행
//!
//! 흐름: [`download`] (크기·SHA-256 확인) → [`stage`] (대상 옆 난수 0700 폴더에 풀고 묶음 검사)
//! → 앱이 [`spawn_apply`] 후 종료 → 도우미 [`run_apply`] 가 파이프 EOF(= 앱 종료)를 보고
//! rename 두 번으로 바꿔 끼우고 `open` 으로 다시 실행. 실패하면 옛 묶음을 되돌린다.

use crate::manifest::{Release, hex};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const BUNDLE_ID: &str = "com.kaifacun.kaiiv";
const STAGE_PREFIX: &str = ".KaiIV-update-";
const CURL: &str = "/usr/bin/curl";
/// 이보다 오래된 단계 폴더만 치운다
const STALE_AFTER: Duration = Duration::from_secs(600);

fn random_hex(n: usize) -> Result<String, String> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(|e| format!("난수 실패: {e}"))?;
    Ok(hex(&b))
}

/// curl 공통 인자 — `-q` 는 첫 인자여야 사용자 ~/.curlrc 를 무시한다
fn curl() -> Command {
    let mut c = Command::new(CURL);
    c.args(["-q", "--fail", "--silent", "--show-error", "--location", "--proto", "=https", "--proto-redir", "=https", "--max-redirs", "5", "--connect-timeout", "15", "-A", "KaiIV-updater"]);
    c
}

/// 시험 빌드만 file:// 피드·파일 허용 (릴리스 빌드는 https 만)
fn local_path(url: &str) -> Option<&Path> {
    if cfg!(debug_assertions) { url.strip_prefix("file://").map(Path::new) } else { None }
}

//#region 피드 받기
/// 피드 원문. 없음(404 등)도 `Err` — 백그라운드 확인은 조용히 넘긴다.
pub fn fetch_feed(url: &str) -> Result<Vec<u8>, String> {
    if let Some(p) = local_path(url) {
        return std::fs::read(p).map_err(|e| format!("피드를 읽을 수 없습니다: {e}"));
    }
    if !url.starts_with("https://") {
        return Err("피드 주소는 https 여야 합니다".into());
    }
    let out = curl().args(["--max-time", "30", "--max-filesize", "65536"]).arg(url).output().map_err(|e| format!("curl 실행 실패: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(format!("업데이트 정보를 받지 못했습니다 ({})", msg.trim().chars().take(160).collect::<String>()));
    }
    Ok(out.stdout)
}
//#endregion

//#region 받기 — 0600 새 파일, 스트리밍 상한, 끝나면 크기·해시
pub fn sha256_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// `work` 폴더에 받는다. `progress(받은 바이트)` 는 수시로 불린다.
pub fn download(rel: &Release, work: &Path, progress: &dyn Fn(u64)) -> Result<PathBuf, String> {
    std::fs::create_dir_all(work).map_err(|e| format!("작업 폴더를 만들 수 없습니다: {e}"))?;
    let dest = work.join(format!("KaiIV-{}.zip", rel.version));
    let _ = std::fs::remove_file(&dest);
    // 새 파일을 0600 으로 먼저 만든다 (curl 은 그 파일에 덮어 쓴다)
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&dest).map_err(|e| format!("받을 파일을 만들 수 없습니다: {e}"))?;
    if let Some(src) = local_path(&rel.url) {
        std::fs::copy(src, &dest).map_err(|e| format!("받기 실패: {e}"))?;
    } else {
        let mut child = curl()
            .args(["--max-time", "900", "--max-filesize"])
            .arg(rel.size.to_string())
            .arg("-o")
            .arg(&dest)
            .arg(&rel.url)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("curl 실행 실패: {e}"))?;
        loop {
            let got = std::fs::metadata(&dest).map_or(0, |m| m.len());
            progress(got);
            // Content-Length 가 없을 때를 위한 스트리밍 상한
            if got > rel.size {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&dest);
                return Err("받은 파일이 예고보다 큽니다 — 버렸습니다".into());
            }
            match child.try_wait() {
                Ok(Some(st)) if st.success() => break,
                Ok(Some(_)) => {
                    let mut e = String::new();
                    let _ = child.stderr.take().map(|mut s| s.read_to_string(&mut e));
                    let _ = std::fs::remove_file(&dest);
                    return Err(format!("받기 실패 ({})", e.trim().chars().take(160).collect::<String>()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(150)),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    let len = std::fs::metadata(&dest).map_err(|e| e.to_string())?.len();
    progress(len);
    let fail = |m: &str| {
        let _ = std::fs::remove_file(&dest);
        Err(m.to_string())
    };
    if len != rel.size {
        return fail("받은 파일 크기가 다릅니다 — 버렸습니다");
    }
    if sha256_file(&dest)? != rel.sha256 {
        return fail("받은 파일의 해시가 다릅니다 — 버렸습니다");
    }
    Ok(dest)
}
//#endregion

//#region 설치 대상 — 지금 실행 중인 .app
fn plist_value(app: &Path, key: &str) -> Option<String> {
    let out = Command::new("/usr/bin/plutil").args(["-extract", key, "raw", "-o", "-"]).arg(app.join("Contents/Info.plist")).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 바꿔 끼울 수 있는 설치 위치. 안 되면 사람에게 보일 까닭.
pub fn install_target() -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(t) = std::env::var_os("IV_UPDATE_TARGET") {
        return check_target(Path::new(&t));
    }
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| e.to_string())?;
    let app = exe.parent().filter(|p| p.ends_with("Contents/MacOS")).and_then(|p| p.parent()?.parent()).filter(|p| p.extension().is_some_and(|e| e == "app"));
    let Some(app) = app else {
        return Err("개발 빌드(.app 밖)에서는 자동 설치를 하지 않습니다".into());
    };
    check_target(app)
}

fn check_target(app: &Path) -> Result<PathBuf, String> {
    if app.to_string_lossy().contains("/AppTranslocation/") {
        return Err("앱이 macOS 임시 위치에서 실행 중입니다. KaiIV 를 응용 프로그램 폴더로 옮긴 뒤 다시 여십시오".into());
    }
    let meta = std::fs::symlink_metadata(app).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Err("설치된 앱을 찾을 수 없습니다".into());
    }
    if plist_value(app, "CFBundleIdentifier").as_deref() != Some(BUNDLE_ID) {
        return Err("KaiIV 묶음이 아닙니다".into());
    }
    let parent = app.parent().ok_or("상위 폴더 없음")?;
    // 실제로 만들고 지워 본다 (dmg·읽기 전용·관리자 소유 폴더)
    let probe = parent.join(format!("{STAGE_PREFIX}probe-{}", random_hex(6)?));
    std::fs::create_dir(&probe).map_err(|_| format!("{} 에 쓸 권한이 없어 자동 설치를 할 수 없습니다", parent.display()))?;
    let _ = std::fs::remove_dir(&probe);
    Ok(app.to_path_buf())
}

/// 지난 업데이트가 남긴 단계 폴더 치우기 (앱 시작 때)
pub fn cleanup_stale(target: &Path) {
    let Some(parent) = target.parent() else { return };
    let Ok(rd) = std::fs::read_dir(parent) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        let is_ours = name.to_string_lossy().starts_with(STAGE_PREFIX) && e.file_type().is_ok_and(|t| t.is_dir());
        // 방금 만든 것(이번 실행의 받기·도우미가 아직 쓰는 중)은 두고, 10분 넘은 것만
        let stale = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d >= STALE_AFTER);
        if is_ours && stale {
            log::info!("지난 업데이트 단계 폴더 치움: {}", e.path().display());
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}
//#endregion

//#region 풀어 검사 — 대상 옆 난수 0700 폴더
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    pub dir: PathBuf,
    pub token: String,
    pub version: crate::Version,
}

fn walk_links_inside(root: &Path, dir: &Path) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        let ft = e.file_type().map_err(|e| e.to_string())?;
        if ft.is_symlink() {
            // 묶음 안을 가리키는 링크만 (Framework 링크 등)
            let real = p.canonicalize().map_err(|_| format!("깨진 링크: {}", p.display()))?;
            if !real.starts_with(root) {
                return Err(format!("묶음 밖을 가리키는 링크: {}", p.display()));
            }
        } else if ft.is_dir() {
            walk_links_inside(root, &p)?;
        }
    }
    Ok(())
}

pub fn stage(zip: &Path, rel: &Release, target: &Path) -> Result<Staged, String> {
    let parent = target.parent().ok_or("상위 폴더 없음")?;
    let dir = parent.join(format!("{STAGE_PREFIX}{}", random_hex(8)?));
    std::fs::create_dir(&dir).map_err(|e| format!("단계 폴더를 만들 수 없습니다: {e}"))?;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let r = stage_into(zip, rel, &dir);
    if r.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    r
}

fn stage_into(zip: &Path, rel: &Release, dir: &Path) -> Result<Staged, String> {
    let x = dir.join("x");
    let st = Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(zip).arg(&x).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("압축을 풀 수 없습니다".into());
    }
    let names: Vec<_> = std::fs::read_dir(&x).map_err(|e| e.to_string())?.flatten().map(|e| e.file_name()).collect();
    if names.len() != 1 || names[0] != "KaiIV.app" {
        return Err("압축 안 내용이 KaiIV.app 하나가 아닙니다".into());
    }
    let app = x.join("KaiIV.app");
    if !std::fs::symlink_metadata(&app).is_ok_and(|m| m.is_dir()) {
        return Err("KaiIV.app 이 폴더가 아닙니다".into());
    }
    let root = app.canonicalize().map_err(|e| e.to_string())?;
    walk_links_inside(&root, &app)?;
    if plist_value(&app, "CFBundleIdentifier").as_deref() != Some(BUNDLE_ID) {
        return Err("묶음 ID 가 다릅니다".into());
    }
    if plist_value(&app, "CFBundleShortVersionString").as_deref() != Some(rel.version.to_string().as_str()) {
        return Err("묶음 판 번호가 서명된 판과 다릅니다".into());
    }
    if !std::fs::metadata(app.join("Contents/MacOS/kaiiv")).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
        return Err("실행 파일이 없습니다".into());
    }
    if !runnable_here(&app.join("Contents/MacOS/kaiiv")) {
        return Err(format!("새 판에 이 맥({})용 실행 파일이 없습니다", std::env::consts::ARCH));
    }
    let cs = Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&app).output().map_err(|e| e.to_string())?;
    if !cs.status.success() {
        return Err("코드 서명 검사에 실패했습니다".into());
    }
    std::fs::rename(&app, dir.join("new.app")).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&x);
    let token = random_hex(16)?;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(dir.join("ticket")).and_then(|mut f| std::io::Write::write_all(&mut f, token.as_bytes())).map_err(|e| e.to_string())?;
    Ok(Staged { dir: dir.to_path_buf(), token, version: rel.version })
}
//#endregion

/// 두 경로를 한 번에 맞바꾼다 (macOS `renamex_np(RENAME_SWAP)`) — 대상 경로가 잠시도 비지 않는다
fn swap(a: &Path, b: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let ca = std::ffi::CString::new(a.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let cb = std::ffi::CString::new(b.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: 두 C 문자열은 호출 동안 살아 있다
    let r = unsafe { libc::renamex_np(ca.as_ptr(), cb.as_ptr(), libc::RENAME_SWAP) };
    if r == 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
}

/// 이 맥에서 돌 수 있는 조각이 실행 파일에 있는가 (`lipo -archs`)
fn runnable_here(exe: &Path) -> bool {
    let want = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        a => a,
    };
    Command::new("/usr/bin/lipo").arg("-archs").arg(exe).output().is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).split_whitespace().any(|a| a == want || (want == "arm64" && a == "arm64e")))
}

//#region 적용 — 앱이 도우미를 띄우고 종료, 도우미가 파이프 EOF 를 보고 바꿔 끼운다
pub const APPLY_FLAG: &str = "--apply-update";

/// 도우미를 띄운다. 이 뒤 앱은 곧 종료해야 한다 (쓰기 끝은 프로세스가 끝날 때 닫힌다).
pub fn spawn_apply(staged: &Staged, target: &Path, log: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .arg(APPLY_FLAG)
        .arg(&staged.dir)
        .arg(target)
        .arg(&staged.token)
        .arg(log)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("업데이트 도우미를 띄울 수 없습니다: {e}"))?;
    // 쓰기 끝을 일부러 놓지 않는다 — 앱 프로세스가 끝나야 닫혀 EOF 가 간다
    std::mem::forget(child.stdin.take());
    Ok(())
}

struct Log(Option<std::fs::File>);

impl Log {
    fn open(p: &Path) -> Self {
        let _ = p.parent().map(std::fs::create_dir_all);
        Log(std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(p).ok())
    }
    fn line(&mut self, s: &str) {
        if let Some(f) = &mut self.0 {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let _ = std::io::Write::write_all(f, format!("{t} {s}\n").as_bytes());
        }
    }
}

/// 도우미 본체 — `kaiiv --apply-update <단계 폴더> <대상> <표> <로그>`. 돌려주는 값은 종료 코드.
pub fn run_apply(args: &[String]) -> i32 {
    let [dir, target, token, log] = args else { return 2 };
    let (dir, target) = (PathBuf::from(dir), PathBuf::from(target));
    let mut log = Log::open(Path::new(log));
    log.line(&format!("적용 시작: {}", target.display()));
    match apply(&dir, &target, token, &mut log, Duration::from_secs(30)) {
        Ok(()) => {
            log.line("적용 끝");
            0
        }
        Err(e) => {
            log.line(&format!("적용 실패: {e}"));
            1
        }
    }
}

fn not_link_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// 대상과 단계 폴더가 짝이 맞는가 — 인자를 그대로 믿지 않는다
fn check_ticket(dir: &Path, target: &Path, token: &str) -> Result<(), String> {
    let name_ok = dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with(STAGE_PREFIX));
    if !name_ok || dir.parent() != target.parent() || !not_link_dir(dir) || !not_link_dir(target) || !not_link_dir(&dir.join("new.app")) {
        return Err("단계 폴더·대상이 맞지 않습니다".into());
    }
    let t = std::fs::read_to_string(dir.join("ticket")).map_err(|e| e.to_string())?;
    if token.len() != 32 || t != token {
        return Err("표가 맞지 않습니다".into());
    }
    if plist_value(target, "CFBundleIdentifier").as_deref() != Some(BUNDLE_ID) || plist_value(&dir.join("new.app"), "CFBundleIdentifier").as_deref() != Some(BUNDLE_ID) {
        return Err("묶음 ID 가 맞지 않습니다".into());
    }
    Ok(())
}

fn apply(dir: &Path, target: &Path, token: &str, log: &mut Log, wait: Duration) -> Result<(), String> {
    check_ticket(dir, target, token)?;
    //#region 앱 종료 기다리기 — 표준 입력 EOF (pid 재사용 걱정 없음)
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut sink);
        let _ = tx.send(());
    });
    rx.recv_timeout(wait).map_err(|_| "앱이 끝나지 않아 교체를 그만둡니다".to_string())?;
    //#endregion
    check_ticket(dir, target, token)?;
    // 맞바꾸기: 대상 ← 새 앱, dir/new.app ← 옛 앱 (경로가 비는 틈 없음)
    let staged = dir.join("new.app");
    swap(&staged, target).map_err(|e| format!("앱을 바꿔 끼울 수 없습니다: {e}"))?;
    log.line("교체 완료(원자적 맞바꿈), 다시 실행 요청");
    // 시험 빌드: IV_UPDATE_NO_OPEN 이면 실제로 열지 않는다 (시험 앱이 사용자 홈을 열지 않게)
    let opened = if cfg!(debug_assertions) && std::env::var_os("IV_UPDATE_NO_OPEN").is_some() {
        log.line("시험: open 생략");
        true
    } else {
        Command::new("/usr/bin/open").arg(target).status().is_ok_and(|s| s.success())
    };
    if !opened {
        // 열기 요청조차 실패하면 다시 맞바꿔 옛 앱으로 되돌려 연다
        let back = swap(&staged, target);
        log.line(&format!("새 앱 열기 요청 실패 → 옛 앱으로 되돌림 {}", if back.is_ok() { "성공" } else { "실패" }));
        let _ = Command::new("/usr/bin/open").arg(target).status();
        return Err("새 앱을 열 수 없어 되돌렸습니다".into());
    }
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}
//#endregion

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iv-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fake_app(at: &Path, version: &str) {
        std::fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        std::fs::write(
            at.join("Contents/Info.plist"),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string><key>CFBundleShortVersionString</key><string>{version}</string><key>CFBundleExecutable</key><string>kaiiv</string></dict></plist>"#),
        )
        .unwrap();
        let exe = at.join("Contents/MacOS/kaiiv");
        // 진짜 Mach-O 여야 아키텍처 검사를 지난다
        std::fs::copy("/usr/bin/true", &exe).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Command::new("/usr/bin/codesign").args(["-s", "-", "--force"]).arg(at).status().unwrap().success());
    }

    fn zip_of(app_parent: &Path, out: &Path) {
        assert!(Command::new("/usr/bin/ditto").args(["-c", "-k", "--keepParent"]).arg(app_parent.join("KaiIV.app")).arg(out).status().unwrap().success());
    }

    fn release_for(zip: &Path, v: &str) -> Release {
        Release { version: crate::Version::parse(v).unwrap(), url: format!("file://{}", zip.display()), size: std::fs::metadata(zip).unwrap().len(), sha256: sha256_file(zip).unwrap(), min_os: "12.0".into(), notes: String::new() }
    }

    #[test]
    fn download_checks_size_and_hash() {
        let d = tmp("dl");
        let src = d.join("a.zip");
        std::fs::write(&src, b"hello").unwrap();
        let mut rel = release_for(&src, "0.4.24");
        assert!(download(&rel, &d.join("w"), &|_| {}).is_ok());
        rel.sha256 = "0".repeat(64);
        assert!(download(&rel, &d.join("w"), &|_| {}).unwrap_err().contains("해시"));
        rel.size = 4;
        assert!(download(&rel, &d.join("w"), &|_| {}).unwrap_err().contains("크기"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stage_and_swap() {
        let d = tmp("swap");
        let apps = d.join("Applications");
        fake_app(&apps.join("KaiIV.app"), "0.4.23");
        let build = d.join("build");
        fake_app(&build.join("KaiIV.app"), "0.4.24");
        let zip = d.join("KaiIV-0.4.24.zip");
        zip_of(&build, &zip);
        let target = check_target(&apps.join("KaiIV.app")).unwrap();
        let rel = release_for(&zip, "0.4.24");

        // 판이 다르면 거부, 단계 폴더도 남기지 않는다
        let mut wrong = rel.clone();
        wrong.version = crate::Version(0, 4, 25);
        assert!(stage(&zip, &wrong, &target).unwrap_err().contains("판"));
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 1);

        let st = stage(&zip, &rel, &target).unwrap();
        assert!(st.dir.join("new.app").is_dir());
        // 표가 틀리면 교체 안 함
        let mut log = Log(None);
        assert!(apply(&st.dir, &target, &"0".repeat(32), &mut log, Duration::from_millis(10)).is_err());
        assert_eq!(plist_value(&target, "CFBundleShortVersionString").as_deref(), Some("0.4.23"));
        // 교체 자체 (open 단계 전까지) — 시험에서는 앱을 실제로 열지 않는다
        swap(&st.dir.join("new.app"), &target).unwrap();
        assert_eq!(plist_value(&target, "CFBundleShortVersionString").as_deref(), Some("0.4.24"));
        // 되돌리기도 같은 맞바꿈
        assert_eq!(plist_value(&st.dir.join("new.app"), "CFBundleShortVersionString").as_deref(), Some("0.4.23"));
        // 방금 만든 단계 폴더는 치우지 않는다 (이번 실행이 쓰는 중일 수 있음)
        cleanup_stale(&target);
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_link_escape_and_extra_entries() {
        let d = tmp("evil");
        let apps = d.join("Applications");
        fake_app(&apps.join("KaiIV.app"), "0.4.23");
        let build = d.join("build");
        fake_app(&build.join("KaiIV.app"), "0.4.24");
        std::os::unix::fs::symlink("/etc", build.join("KaiIV.app/Contents/Resources")).unwrap();
        let zip = d.join("evil.zip");
        zip_of(&build, &zip);
        let rel = release_for(&zip, "0.4.24");
        let target = check_target(&apps.join("KaiIV.app")).unwrap();
        assert!(stage(&zip, &rel, &target).unwrap_err().contains("링크"));
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_foreign_arch() {
        let d = tmp("arch");
        let apps = d.join("Applications");
        fake_app(&apps.join("KaiIV.app"), "0.4.23");
        let build = d.join("build");
        fake_app(&build.join("KaiIV.app"), "0.4.24");
        // 이 맥이 아닌 아키텍처만 남긴다
        let other = if std::env::consts::ARCH == "aarch64" { "x86_64" } else { "arm64e" };
        let exe = build.join("KaiIV.app/Contents/MacOS/kaiiv");
        assert!(Command::new("/usr/bin/lipo").arg(&exe).args(["-thin", other, "-output"]).arg(&exe).status().unwrap().success());
        assert!(Command::new("/usr/bin/codesign").args(["-s", "-", "--force"]).arg(build.join("KaiIV.app")).status().unwrap().success());
        let zip = d.join("x.zip");
        zip_of(&build, &zip);
        let target = check_target(&apps.join("KaiIV.app")).unwrap();
        assert!(stage(&zip, &release_for(&zip, "0.4.24"), &target).unwrap_err().contains("용 실행 파일이 없습니다"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn target_rules() {
        assert!(check_target(Path::new("/private/var/folders/x/AppTranslocation/y/KaiIV.app")).unwrap_err().contains("임시 위치"));
        let d = tmp("tgt");
        std::fs::create_dir_all(d.join("Other.app/Contents")).unwrap();
        assert!(check_target(&d.join("Other.app")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
