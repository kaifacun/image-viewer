//! 업데이트 설정·기억 — `auto=1`, `last_check=<유닉스 초>`, `skip=<판>` 줄. 원자적 교체·0600.

use crate::Version;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// 자동 확인 간격
pub const CHECK_EVERY_SECS: u64 = 24 * 3600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub auto: bool,
    pub last_check: u64,
    pub skip: Option<Version>,
    /// 지금까지 확인한 가장 높은 서명된 판 — 이보다 낮은(철회된) 판의 재생은 제안하지 않는다
    pub highest: Option<Version>,
}

impl Default for State {
    fn default() -> Self {
        State { auto: true, last_check: 0, skip: None, highest: None }
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl State {
    /// ~/Library/Application Support/KaiIV/update-state
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/KaiIV/update-state"))
    }

    pub fn load(p: &Path) -> Self {
        let mut s = State::default();
        let Ok(t) = std::fs::read_to_string(p) else { return s };
        for line in t.lines() {
            match line.split_once('=') {
                Some(("auto", v)) => s.auto = v.trim() != "0",
                Some(("last_check", v)) => s.last_check = v.trim().parse().unwrap_or(0),
                Some(("skip", v)) => s.skip = Version::parse(v.trim()),
                Some(("highest", v)) => s.highest = Version::parse(v.trim()),
                _ => {}
            }
        }
        s
    }

    pub fn save(&self, p: &Path) -> std::io::Result<()> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("tmp");
        let opt = |v: Option<Version>| v.map(|v| v.to_string()).unwrap_or_default();
        let body = format!("auto={}\nlast_check={}\nskip={}\nhighest={}\n", u8::from(self.auto), self.last_check, opt(self.skip), opt(self.highest));
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        std::io::Write::write_all(&mut f, body.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(tmp, p)
    }

    /// 자동 확인할 때인가
    pub fn due(&self, now: u64) -> bool {
        self.auto && now.saturating_sub(self.last_check) >= CHECK_EVERY_SECS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_due() {
        let d = std::env::temp_dir().join(format!("iv-update-state-{}", std::process::id()));
        let p = d.join("update-state");
        assert_eq!(State::load(&p), State::default());
        let s = State { auto: false, last_check: 100, skip: Some(Version(0, 4, 24)), highest: Some(Version(0, 4, 25)) };
        s.save(&p).unwrap();
        assert_eq!(State::load(&p), s);
        assert!(!s.due(10_000_000));
        let s = State { auto: true, ..s };
        assert!(!s.due(100 + 3600));
        assert!(s.due(100 + CHECK_EVERY_SECS));
        let _ = std::fs::remove_dir_all(&d);
    }
}
