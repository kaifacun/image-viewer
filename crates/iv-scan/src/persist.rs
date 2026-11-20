//! 스캔 결과 저장·불러오기 — 다음 실행 때 전부 다시 나열하지 않고 폴더 수정 시각만 확인하기 위해.
//!
//! 한 줄 = 한 폴더: `경로 \t 초 \t 나노 \t 묶음별사진수(,) \t 모르는하위 \t 표식(16진) \t 확신못함 \t 하위이름들(\x1f) \t 링크대상들(\x1f)`.
//! 첫 줄은 머리(형식 버전 + 형식 묶음 서명) — 다르면 통째로 버린다 (묶음이 바뀌면 수·표식이 달라진다).

use std::collections::HashMap;
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Found;

const MAGIC: &str = "kaiiv-scan-v3"; // v2: Finder 숨김 폴더 제외, v3: 형식 묶음별 수·표식
const SEP: char = '\u{1f}';

/// 폴더 하나의 나열 결과 + 판정 (캐시 단위)
#[derive(Clone, Debug)]
pub(crate) struct Cached {
    pub mtime: SystemTime,
    pub counts: Vec<u32>,
    pub unknown_child: bool,
    pub subdirs: Vec<PathBuf>,
    pub links: Vec<PathBuf>,
    /// 공개한 결과 (하위까지)
    pub found: Found,
}

fn bad(s: &str) -> bool {
    s.contains(['\t', '\n', '\r', SEP])
}

pub(crate) fn load(file: &Path, sig: &str) -> HashMap<PathBuf, Cached> {
    let mut out = HashMap::new();
    let Ok(f) = std::fs::File::open(file) else { return out };
    let mut lines = std::io::BufReader::new(f).lines();
    match lines.next() {
        Some(Ok(h)) if h == format!("{MAGIC}\t{sig}") => {}
        _ => return out,
    }
    for line in lines.map_while(Result::ok) {
        let c: Vec<&str> = line.split('\t').collect();
        let [path, secs, nanos, counts, unk, mask, uncertain, subs, links] = c[..] else { continue };
        let (Ok(secs), Ok(nanos), Ok(mask)) = (secs.parse::<u64>(), nanos.parse::<u32>(), u64::from_str_radix(mask, 16)) else { continue };
        let Ok(counts) = counts.split(',').filter(|x| !x.is_empty()).map(str::parse::<u32>).collect::<Result<Vec<_>, _>>() else { continue };
        let dir = PathBuf::from(path);
        let split = |s: &str| s.split(SEP).filter(|x| !x.is_empty()).map(String::from).collect::<Vec<_>>();
        let cached = Cached {
            mtime: UNIX_EPOCH + Duration::new(secs, nanos),
            counts,
            unknown_child: unk == "1",
            subdirs: split(subs).into_iter().map(|n| dir.join(n)).collect(),
            links: split(links).into_iter().map(PathBuf::from).collect(),
            found: Found { mask, unknown: uncertain == "1" },
        };
        out.insert(dir, cached);
    }
    out
}

/// 임시 파일에 쓰고 바꿔치기 (쓰다 끊겨도 옛 파일이 남는다)
pub(crate) fn save(file: &Path, sig: &str, map: &HashMap<PathBuf, Cached>) -> std::io::Result<()> {
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = file.with_extension("tmp");
    {
        let mut w = BufWriter::new(std::fs::File::create(&tmp)?);
        writeln!(w, "{MAGIC}\t{sig}")?;
        for (dir, c) in map {
            let Some(path) = dir.to_str().filter(|p| !bad(p)) else { continue };
            let Ok(t) = c.mtime.duration_since(UNIX_EPOCH) else { continue };
            let names: Option<Vec<&str>> = c.subdirs.iter().map(|p| p.file_name().and_then(|n| n.to_str()).filter(|n| !bad(n))).collect();
            let links: Option<Vec<&str>> = c.links.iter().map(|p| p.to_str().filter(|n| !bad(n))).collect();
            let (Some(names), Some(links)) = (names, links) else { continue };
            let sep = SEP.to_string();
            let counts = c.counts.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            writeln!(w, "{path}\t{}\t{}\t{counts}\t{}\t{:x}\t{}\t{}\t{}", t.as_secs(), t.subsec_nanos(), u8::from(c.unknown_child), c.found.mask, u8::from(c.found.unknown), names.join(&sep), links.join(&sep))?;
        }
        w.flush()?;
    }
    std::fs::rename(&tmp, file)
}
