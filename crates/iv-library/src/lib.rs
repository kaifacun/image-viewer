//! 폴더의 이미지 목록과 현재 위치.
//!
//! 확장자 필터는 후보 축소 힌트일 뿐이며, 실제 지원 여부는 디코드 시 레지스트리가 판정한다.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use iv_core::ImageSource;

/// 파일 하나의 안정 식별자 — 경로 + 수정시각 + 크기 지문.
/// 파일이 바뀌면 id 가 바뀌어 낡은 캐시가 자연히 쓰이지 않는다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryId(pub u64);

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: EntryId,
    pub source: ImageSource,
    pub name: String,
    /// 파일 크기 (바이트)
    pub len: u64,
    pub mtime: SystemTime,
}

impl Entry {
    pub fn from_path(path: PathBuf) -> Self {
        let meta = std::fs::metadata(&path).ok();
        let mtime = meta.as_ref().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
        let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut h);
        mtime.hash(&mut h);
        len.hash(&mut h);
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Self { id: EntryId(h.finish()), source: ImageSource::Path(path), name, len, mtime }
    }

    pub fn path(&self) -> Option<&Path> {
        match &self.source {
            ImageSource::Path(p) => Some(p),
        }
    }
}

/// 정렬 기준
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Size,
    Type,
}

#[derive(Default)]
pub struct Library {
    entries: Vec<Entry>,
    index: usize,
    dir: Option<PathBuf>,
    /// 하위 폴더 수 (상태줄)
    subdirs: usize,
    sort: (SortKey, bool),
}

impl Library {
    /// 파일 또는 폴더를 연다. 파일이면 같은 폴더를 스캔하고 그 파일에 위치한다.
    pub fn open(path: &Path, extensions: &HashSet<&'static str>) -> std::io::Result<Self> {
        let path = path.canonicalize()?;
        let (dir, start) = if path.is_dir() { (path.clone(), None) } else { (path.parent().unwrap_or(Path::new("/")).to_path_buf(), Some(path.clone())) };
        let mut subdirs = 0;
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                if p.is_dir() {
                    subdirs += usize::from(!is_hidden(p));
                    return false;
                }
                p.is_file() && !is_hidden(p) && has_ext(p, extensions)
            })
            .collect();
        // 열려는 파일이 확장자 목록에 없어도(예: 확장자 없는 파일) 목록에 넣는다
        if let Some(s) = &start {
            if !files.contains(s) {
                files.push(s.clone());
            }
        }
        files.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
        let index = start.and_then(|s| files.iter().position(|f| *f == s)).unwrap_or(0);
        let entries = files.into_iter().map(Entry::from_path).collect();
        Ok(Self { entries, index, dir: Some(dir), subdirs, sort: (SortKey::Name, true) })
    }

    /// 빈 목록 (폴더를 열지 못했을 때 — 트리와 격자가 어긋나지 않게 폴더만 기억)
    pub fn empty_at(dir: PathBuf) -> Self {
        Self { entries: Vec::new(), index: 0, dir: Some(dir), subdirs: 0, sort: (SortKey::Name, true) }
    }

    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self { entries, index: 0, dir: None, subdirs: 0, sort: (SortKey::Name, true) }
    }

    /// 다시 정렬. 현재 항목은 그대로 따라간다.
    pub fn sort(&mut self, key: SortKey, ascending: bool) {
        let cur = self.current().map(|e| e.id);
        let ext = |e: &Entry| e.name.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default();
        self.entries.sort_by(|a, b| {
            let o = match key {
                SortKey::Name => std::cmp::Ordering::Equal,
                SortKey::Modified => a.mtime.cmp(&b.mtime),
                SortKey::Size => a.len.cmp(&b.len),
                SortKey::Type => ext(a).cmp(&ext(b)),
            }
            .then_with(|| natural_cmp(&a.name, &b.name));
            if ascending { o } else { o.reverse() }
        });
        self.sort = (key, ascending);
        if let Some(id) = cur {
            self.index = self.position(id).unwrap_or(0);
        }
    }

    pub fn sort_order(&self) -> (SortKey, bool) {
        self.sort
    }

    pub fn position(&self, id: EntryId) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn subdir_count(&self) -> usize {
        self.subdirs
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.len).sum()
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.index)
    }
    pub fn get(&self, i: usize) -> Option<&Entry> {
        self.entries.get(i)
    }

    /// 상대 이동. 끝에서 멈춘다(순환하지 않음). 위치가 바뀌었으면 true.
    pub fn step(&mut self, delta: isize) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let new = (self.index as isize + delta).clamp(0, self.entries.len() as isize - 1) as usize;
        let changed = new != self.index;
        self.index = new;
        changed
    }

    pub fn go_to(&mut self, i: usize) -> bool {
        if i >= self.entries.len() || i == self.index {
            return false;
        }
        self.index = i;
        true
    }

    /// 열 수 없는 항목을 목록에서 뺀다 (깨진 파일을 넘기며 계속 마주치지 않게)
    pub fn remove(&mut self, id: EntryId) {
        if let Some(pos) = self.entries.iter().position(|e| e.id == id) {
            self.entries.remove(pos);
            if pos < self.index || self.index >= self.entries.len() {
                self.index = self.index.saturating_sub(1);
            }
        }
    }

    /// 여러 항목을 한꺼번에 뺀다 (휴지통 이동 후). 현재 위치는 지운 것 중 첫 자리에 남은 항목
    /// (Finder 처럼 "다음 장"), 끝을 넘으면 마지막 항목. 하나라도 뺐으면 true.
    pub fn remove_ids(&mut self, ids: &HashSet<EntryId>) -> bool {
        let Some(first) = self.entries.iter().position(|e| ids.contains(&e.id)) else { return false };
        self.entries.retain(|e| !ids.contains(&e.id));
        self.index = first.min(self.entries.len().saturating_sub(1));
        true
    }

    /// 현재 위치 기준 이웃: (앞으로 n개, 뒤로 m개) — 가까운 순
    pub fn neighbors(&self, ahead: usize, behind: usize) -> (Vec<&Entry>, Vec<&Entry>) {
        let a = (1..=ahead).filter_map(|k| self.entries.get(self.index + k)).collect();
        let b = (1..=behind).filter_map(|k| self.index.checked_sub(k).and_then(|i| self.entries.get(i))).collect();
        (a, b)
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn is_hidden(p: &Path) -> bool {
    p.file_name().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with('.'))
}

fn has_ext(p: &Path, exts: &HashSet<&'static str>) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.contains(e.to_ascii_lowercase().as_str()))
}

/// 자연 정렬: "img2" < "img10", 대소문자 무시 (Finder 와 같은 감각)
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na: String = std::iter::from_fn(|| ai.next_if(|c| c.is_ascii_digit())).collect();
                let nb: String = std::iter::from_fn(|| bi.next_if(|c| c.is_ascii_digit())).collect();
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["img10.jpg", "img2.jpg", "IMG1.jpg", "img02b.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["IMG1.jpg", "img2.jpg", "img02b.jpg", "img10.jpg"]);
    }

    #[test]
    fn step_clamps_and_neighbors() {
        let es = (0..5).map(|i| Entry::from_path(PathBuf::from(format!("/tmp/x{i}.jpg")))).collect();
        let mut l = Library::from_entries(es);
        assert!(!l.step(-1));
        assert!(l.step(3));
        let (a, b) = l.neighbors(3, 2);
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].name, "x2.jpg");
        assert!(!l.step(10) || l.index() == 4);
    }

    #[test]
    fn remove_ids_lands_on_next_survivor() {
        let es: Vec<Entry> = (0..6).map(|i| Entry::from_path(PathBuf::from(format!("/tmp/r{i}.jpg")))).collect();
        let ids: Vec<EntryId> = es.iter().map(|e| e.id).collect();
        let mut l = Library::from_entries(es);
        l.go_to(4);
        // r1, r2 삭제 → 첫 삭제 자리(1)에 r3
        assert!(l.remove_ids(&HashSet::from([ids[1], ids[2]])));
        assert_eq!(l.current().unwrap().name, "r3.jpg");
        // 마지막 두 장 삭제 → 끝으로 당겨짐
        assert!(l.remove_ids(&HashSet::from([ids[4], ids[5]])));
        assert_eq!(l.current().unwrap().name, "r3.jpg");
        assert!(!l.remove_ids(&HashSet::from([ids[1]])));
        // 전부 삭제 → 빈 목록
        assert!(l.remove_ids(&HashSet::from([ids[0], ids[3]])));
        assert!(l.is_empty() && l.current().is_none());
    }

    #[test]
    fn sort_keeps_current() {
        let mut es: Vec<Entry> = (0..4).map(|i| Entry::from_path(PathBuf::from(format!("/tmp/x{i}.jpg")))).collect();
        for (i, e) in es.iter_mut().enumerate() {
            e.len = [30, 10, 40, 20][i];
        }
        let mut l = Library::from_entries(es);
        l.go_to(2); // x2 (40)
        l.sort(SortKey::Size, true);
        let names: Vec<_> = l.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["x1.jpg", "x3.jpg", "x0.jpg", "x2.jpg"]);
        assert_eq!(l.current().unwrap().name, "x2.jpg");
        l.sort(SortKey::Name, false);
        assert_eq!(l.entries()[0].name, "x3.jpg");
        assert_eq!(l.current().unwrap().name, "x2.jpg");
    }
}
