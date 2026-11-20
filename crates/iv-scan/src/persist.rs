//! 스캔 결과 저장·불러오기 — 다음 실행 때 전부 다시 나열하지 않고 폴더 수정 시각만 확인하기 위해.
//!
//! 한 줄 = 한 폴더: `경로 \t 초 \t 나노 \t 사진수 \t 모름여부 \t 판정(H/E/U) \t 보일하위수 \t 하위이름들(\x1f) \t 링크대상들(\x1f)`.
//! 첫 줄은 머리(형식 버전 + 사진 확장자 서명) — 다르면 통째로 버린다 (확장자가 늘면 사진 수가 달라진다).

use std::collections::HashMap;
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Verdict;

const MAGIC: &str = "kaiiv-scan-v1";
const SEP: char = '\u{1f}';

/// 폴더 하나의 나열 결과 + 판정 (캐시 단위)
#[derive(Clone, Debug)]
pub(crate) struct Cached {
    pub mtime: SystemTime,
    pub photos: u64,
    pub unknown_child: bool,
    pub subdirs: Vec<PathBuf>,
    pub links: Vec<PathBuf>,
    pub verdict: Verdict,
    pub folders: u32,
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
        let [path, secs, nanos, photos, unk, verdict, folders, subs, links] = c[..] else { continue };
        let (Ok(secs), Ok(nanos), Ok(photos), Ok(folders)) = (secs.parse::<u64>(), nanos.parse::<u32>(), photos.parse::<u64>(), folders.parse::<u32>()) else { continue };
        let dir = PathBuf::from(path);
        let verdict = match verdict {
            "H" => Verdict::HasPhotos,
            "E" => Verdict::Empty,
            _ => Verdict::Unknown,
        };
        let split = |s: &str| s.split(SEP).filter(|x| !x.is_empty()).map(String::from).collect::<Vec<_>>();
        let cached = Cached {
            mtime: UNIX_EPOCH + Duration::new(secs, nanos),
            photos,
            unknown_child: unk == "1",
            subdirs: split(subs).into_iter().map(|n| dir.join(n)).collect(),
            links: split(links).into_iter().map(PathBuf::from).collect(),
            verdict,
            folders,
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
            let v = match c.verdict {
                Verdict::HasPhotos => "H",
                Verdict::Empty => "E",
                Verdict::Unknown => "U",
            };
            let sep = SEP.to_string();
            writeln!(w, "{path}\t{}\t{}\t{}\t{}\t{v}\t{}\t{}\t{}", t.as_secs(), t.subsec_nanos(), c.photos, u8::from(c.unknown_child), c.folders, names.join(&sep), links.join(&sep))?;
        }
        w.flush()?;
    }
    std::fs::rename(&tmp, file)
}
