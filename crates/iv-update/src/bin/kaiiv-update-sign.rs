//! 배포자 도구 — 업데이트 서명 키 만들기·공개키 보기·update.json 서명.
//!
//!   kaiiv-update-sign keygen                      키체인에 새 비밀키 (이미 있으면 거부)
//!   kaiiv-update-sign pubkey                      공개키(base64) 출력
//!   kaiiv-update-sign sign <zip> <판> <url> <알림글 파일> <출력 update.json>
//!   kaiiv-update-sign export <파일>              키체인의 비밀키를 파일로 (0600, 저장소 안 거부) — USB 로 옮길 때
//!   kaiiv-update-sign import <파일>              그 파일을 이 맥 키체인에 (앱의 PUBLIC_KEY 와 맞아야)
//!
//! 비밀키는 macOS 키체인(서비스 `KaiIV-update-ed25519`)에만 있다 — 화면·파일·인자에 내보내지 않는다.
//! 시험용으로만 `IV_UPDATE_KEY_FILE=<32바이트 base64 파일>` 을 쓸 수 있다.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::SigningKey;
use iv_update::install::sha256_file;
use iv_update::{Manifest, Version};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

const SERVICE: &str = "KaiIV-update-ed25519";
const ACCOUNT: &str = "kaiiv";

/// 키 파일 글 → 비밀키 (`#` 줄은 설명)
fn parse_key_text(t: &str) -> Result<SigningKey, String> {
    let line = t.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).ok_or("키 파일에 키가 없습니다")?;
    let b: [u8; 32] = B64.decode(line).ok().and_then(|v| v.try_into().ok()).ok_or("서명 키 형식이 잘못되었습니다")?;
    Ok(SigningKey::from_bytes(&b))
}

fn keychain_store(seed: &[u8; 32]) -> Result<(), String> {
    // 비밀값은 인자(ps 에 보임)가 아니라 표준 입력으로 security 에 넘긴다
    let mut child = Command::new("/usr/bin/security").arg("-i").stdin(Stdio::piped()).stdout(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let line = format!("add-generic-password -s {SERVICE} -a {ACCOUNT} -l \"KaiIV 업데이트 서명 키\" -w {}\n", B64.encode(seed));
    child.stdin.take().ok_or("stdin")?.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    if !child.wait().map_err(|e| e.to_string())?.success() {
        return Err("키체인 저장 실패".into());
    }
    Ok(())
}

fn load_secret() -> Result<SigningKey, String> {
    if let Some(f) = std::env::var_os("IV_UPDATE_KEY_FILE") {
        return parse_key_text(&std::fs::read_to_string(f).map_err(|e| e.to_string())?);
    }
    let b64 = {
        let out = Command::new("/usr/bin/security").args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"]).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("키체인에 서명 키가 없습니다 (서비스 {SERVICE}) — 먼저 keygen"));
        }
        String::from_utf8(out.stdout).map_err(|e| e.to_string())?
    };
    let b: [u8; 32] = B64.decode(b64.trim()).ok().and_then(|v| v.try_into().ok()).ok_or("서명 키 형식이 잘못되었습니다")?;
    Ok(SigningKey::from_bytes(&b))
}

fn keygen() -> Result<(), String> {
    if load_secret().is_ok() {
        return Err("이미 서명 키가 있습니다 — 덮어쓰지 않습니다 (잃으면 기존 사용자에게 업데이트를 보낼 수 없음)".into());
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
    let key = SigningKey::from_bytes(&seed);
    keychain_store(&seed)?;
    let back = load_secret()?;
    if back.verifying_key() != key.verifying_key() {
        return Err("키체인에서 다시 읽은 키가 다릅니다".into());
    }
    println!("{}", B64.encode(key.verifying_key().to_bytes()));
    eprintln!("키체인에 저장했습니다. 위 공개키를 crates/iv-update/src/lib.rs PUBLIC_KEY 에 넣으십시오.");
    Ok(())
}

fn sign(args: &[String]) -> Result<(), String> {
    let [zip, version, url, notes, out] = args else { return Err("sign <zip> <판> <url> <알림글 파일> <출력>".into()) };
    Version::parse(version).ok_or("판 번호 형식")?;
    let notes = std::fs::read_to_string(notes).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(zip).map_err(|e| e.to_string())?.len();
    let sha = sha256_file(std::path::Path::new(zip))?;
    let key = load_secret()?;
    let m = Manifest::sign(&key, version, url, size, &sha, "12.0", notes.trim_end());
    // 자기 검증 — 앱에 박힌 공개키로 통과해야 한다
    m.verify(&key.verifying_key())?;
    let pk = iv_update::manifest::verifying_key(iv_update::PUBLIC_KEY);
    if pk != Some(key.verifying_key()) && std::env::var_os("IV_UPDATE_KEY_FILE").is_none() {
        return Err("키체인 키가 앱의 PUBLIC_KEY 와 다릅니다 — 이대로 내보내면 앱이 거부합니다".into());
    }
    let json = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
    std::fs::write(out, json + "\n").map_err(|e| e.to_string())?;
    eprintln!("서명함: {version} ({size} 바이트, sha256 {sha})");
    Ok(())
}

/// git 작업 폴더 안인가 (실수로 커밋되지 않게 거부)
fn inside_git(p: &Path) -> bool {
    let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    Command::new("git").arg("-C").arg(dir).args(["rev-parse", "--is-inside-work-tree"]).output().is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
}

fn export(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if inside_git(p) {
        return Err("git 저장소 안에는 내보내지 않습니다 — 저장소 밖 경로를 주십시오".into());
    }
    let key = std::env::var_os("IV_UPDATE_KEY_FILE").map_or_else(load_secret, |_| Err("내보내기는 키체인 키만".into()))?;
    if let Some(d) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        let _ = std::fs::set_permissions(d, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    }
    let body = format!(
        "# KaiIV 업데이트 서명 비밀키 — 저장소·메일·메신저·클라우드에 올리지 말 것\n# 다른 맥: cargo run -p iv-update --bin kaiiv-update-sign -- import <이 파일>\n# public: {}\n{}\n",
        B64.encode(key.verifying_key().to_bytes()),
        B64.encode(key.to_bytes())
    );
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).map_err(|e| format!("{path}: {e} (이미 있으면 덮어쓰지 않습니다)"))?;
    f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    // 다시 읽어 같은 키인지
    if parse_key_text(&std::fs::read_to_string(p).map_err(|e| e.to_string())?)?.verifying_key() != key.verifying_key() {
        return Err("내보낸 파일을 다시 읽은 키가 다릅니다".into());
    }
    eprintln!("내보냄: {path} (0600). 공개키 {}", B64.encode(key.verifying_key().to_bytes()));
    Ok(())
}

fn import(path: &str) -> Result<(), String> {
    let key = parse_key_text(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)?;
    if iv_update::manifest::verifying_key(iv_update::PUBLIC_KEY) != Some(key.verifying_key()) {
        return Err("이 키는 앱의 PUBLIC_KEY 와 맞지 않습니다 — 다른 키입니다".into());
    }
    match load_secret() {
        Ok(k) if k.verifying_key() == key.verifying_key() => {
            eprintln!("이미 같은 키가 키체인에 있습니다");
            return Ok(());
        }
        Ok(_) => return Err("키체인에 다른 서명 키가 있습니다 — 덮어쓰지 않습니다 (키체인 접근 앱에서 먼저 확인)".into()),
        Err(_) => {}
    }
    keychain_store(&key.to_bytes())?;
    if load_secret()?.verifying_key() != key.verifying_key() {
        return Err("키체인에서 다시 읽은 키가 다릅니다".into());
    }
    eprintln!("키체인에 넣었습니다 ({SERVICE}). 이제 tools/make-app.sh --update 로 서명할 수 있습니다. USB 의 파일은 지우거나 안전히 보관하십시오.");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("keygen") => keygen(),
        Some("pubkey") => load_secret().map(|k| println!("{}", B64.encode(k.verifying_key().to_bytes()))),
        Some("sign") => sign(&args[1..]),
        Some("export") if args.len() == 2 => export(&args[1]),
        Some("import") if args.len() == 2 => import(&args[1]),
        _ => Err("사용법: kaiiv-update-sign keygen | pubkey | sign <zip> <판> <url> <알림글 파일> <출력> | export <파일> | import <파일>".into()),
    };
    if let Err(e) = r {
        eprintln!("오류: {e}");
        std::process::exit(1);
    }
}
