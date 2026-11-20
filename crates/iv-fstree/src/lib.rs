//! 폴더 트리 모델.
//!
//! UI 와 무관하다: 노드의 펼침 상태와 하위 폴더 목록(지연 나열·캐시)만 가진다.
//! UI 는 [`FolderTree::visible_rows`] 를 그리고, 사용자의 펼침/접기를 [`FolderTree::toggle`] 로 알린다.
//! v1 은 동기 나열이다 (느린 네트워크 볼륨에서는 UI 가 잠시 멈출 수 있음 — 차후 백그라운드 나열).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// 화면에 그릴 한 줄
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub expanded: bool,
    /// 하위 폴더가 있을 수 있음 (펼침 삼각형 표시). 나열 전이면 true 로 가정.
    pub has_children: bool,
}

#[derive(Default)]
pub struct FolderTree {
    roots: Vec<(PathBuf, String)>,
    expanded: HashSet<PathBuf>,
    /// 경로 → 하위 폴더 (나열 결과 캐시). Err 는 권한 없음 등.
    children: HashMap<PathBuf, Result<Vec<PathBuf>, String>>,
}

impl FolderTree {
    /// 루트 목록으로 만든다. (이름, 경로)
    pub fn new(roots: Vec<(String, PathBuf)>) -> Self {
        Self { roots: roots.into_iter().map(|(n, p)| (p, n)).collect(), ..Default::default() }
    }

    /// 기본 루트: `IV_TREE_ROOTS`(구분자 `:`/`;`) 가 있으면 그것만, 없으면 홈·데스크탑·사진·볼륨(맥)/드라이브(윈도).
    pub fn default_roots() -> Vec<(String, PathBuf)> {
        if let Ok(v) = std::env::var("IV_TREE_ROOTS") {
            let sep = if cfg!(windows) { ';' } else { ':' };
            return v.split(sep).filter(|s| !s.is_empty()).map(|s| (display_name(Path::new(s)), PathBuf::from(s))).collect();
        }
        let mut out = Vec::new();
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
        if let Some(h) = &home {
            out.push(("홈".to_string(), h.clone()));
            for (n, sub) in [("데스크탑", "Desktop"), ("사진", "Pictures"), ("다운로드", "Downloads")] {
                let p = h.join(sub);
                if p.is_dir() {
                    out.push((n.to_string(), p));
                }
            }
        }
        if cfg!(target_os = "macos") {
            if let Ok(rd) = std::fs::read_dir("/Volumes") {
                let mut vols: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
                vols.sort();
                out.extend(vols.into_iter().map(|p| (display_name(&p), p)));
            }
        } else if cfg!(windows) {
            for c in b'C'..=b'Z' {
                let p = PathBuf::from(format!("{}:\\", c as char));
                if p.is_dir() {
                    out.push((format!("{}:", c as char), p));
                }
            }
        } else {
            out.push(("/".to_string(), PathBuf::from("/")));
        }
        out
    }

    pub fn is_expanded(&self, p: &Path) -> bool {
        self.expanded.contains(p)
    }

    /// 펼침/접기 전환. 펼칠 때 처음이면 나열한다.
    pub fn toggle(&mut self, p: &Path) {
        if !self.expanded.remove(p) {
            self.ensure_listed(p);
            self.expanded.insert(p.to_path_buf());
        }
    }

    /// 경로가 보이도록 조상 노드를 모두 펼친다 (폴더를 열었을 때 트리 동기화).
    pub fn expand_to(&mut self, target: &Path) {
        let Some(root) = self.roots.iter().map(|(r, _)| r).filter(|r| target.starts_with(r)).max_by_key(|r| r.as_os_str().len()).cloned() else {
            return;
        };
        let mut cur = root;
        for comp in target.strip_prefix(&cur).map(|r| r.components().collect::<Vec<_>>()).unwrap_or_default() {
            self.ensure_listed(&cur);
            self.expanded.insert(cur.clone());
            cur = cur.join(comp);
        }
    }

    /// 다시 나열 (새로고침)
    pub fn refresh(&mut self) {
        self.children.clear();
        let exp: Vec<PathBuf> = self.expanded.iter().cloned().collect();
        for p in exp {
            self.ensure_listed(&p);
        }
    }

    fn ensure_listed(&mut self, p: &Path) {
        if !self.children.contains_key(p) {
            self.children.insert(p.to_path_buf(), list_subdirs(p));
        }
    }

    /// 나열 오류 (권한 없음 등)
    pub fn error(&self, p: &Path) -> Option<&str> {
        self.children.get(p).and_then(|r| r.as_ref().err()).map(|s| s.as_str())
    }

    /// 루트 경로들 (트리에 보이는 그대로)
    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|(p, _)| p.clone()).collect()
    }

    /// 펼쳐진 상태대로 평탄화한 줄 목록
    pub fn visible_rows(&self) -> Vec<Row> {
        self.visible_rows_where(&|_| false)
    }

    /// `hide` 가 참인 폴더(와 그 하위)는 빼고 평탄화. 숨긴 것만 남은 폴더는 펼침 삼각형도 없다.
    pub fn visible_rows_where(&self, hide: &dyn Fn(&Path) -> bool) -> Vec<Row> {
        let mut out = Vec::new();
        for (p, n) in &self.roots {
            if !hide(p) {
                self.push_rows(p, n.clone(), 0, hide, &mut out);
            }
        }
        out
    }

    fn push_rows(&self, p: &Path, name: String, depth: usize, hide: &dyn Fn(&Path) -> bool, out: &mut Vec<Row>) {
        let expanded = self.expanded.contains(p);
        let kids = self.children.get(p);
        let has_children = match kids {
            None => !hide(p),
            Some(Ok(v)) => v.iter().any(|c| !hide(c)),
            Some(Err(_)) => false,
        };
        out.push(Row { path: p.to_path_buf(), name, depth, expanded, has_children });
        if expanded {
            if let Some(Ok(v)) = kids {
                for c in v.iter().filter(|c| !hide(c)) {
                    self.push_rows(c, display_name(c), depth + 1, hide, out);
                }
            }
        }
    }

    /// 한 폴더의 하위 목록만 다시 읽게 한다 (새 폴더가 생겼을 때)
    pub fn invalidate(&mut self, p: &Path) {
        if self.children.remove(p).is_some() && self.expanded.contains(p) {
            self.ensure_listed(p);
        }
    }
}

/// 숨김이 아닌 하위 폴더, 자연 정렬
pub fn list_subdirs(p: &Path) -> Result<Vec<PathBuf>, String> {
    let rd = std::fs::read_dir(p).map_err(|e| e.to_string())?;
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false) || (e.file_type().map(|t| t.is_symlink()).unwrap_or(false) && e.path().is_dir()))
        .map(|e| e.path())
        .filter(|p| !display_name(p).starts_with('.') && !is_bundle(p))
        .collect();
    v.sort_by_key(|p| display_name(p).to_lowercase());
    Ok(v)
}

/// macOS 패키지(.app 등)는 폴더처럼 보이지 않게
fn is_bundle(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e, "app" | "bundle" | "framework" | "photoslibrary" | "pkg"))
}

pub fn display_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("iv-fstree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for s in ["a/x", "a/y/z", "b", ".hidden", "c.app"] {
            std::fs::create_dir_all(d.join(s)).unwrap();
        }
        std::fs::write(d.join("a/file.jpg"), b"x").unwrap();
        d
    }

    #[test]
    fn lazy_expand_and_expand_to() {
        let d = tmp();
        let mut t = FolderTree::new(vec![("root".into(), d.clone())]);
        let rows = t.visible_rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].has_children && !rows[0].expanded);
        t.toggle(&d);
        let names: Vec<_> = t.visible_rows().iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, ["root", "a", "b"]); // 숨김·번들 제외
        t.expand_to(&d.join("a/y/z"));
        let names: Vec<_> = t.visible_rows().iter().map(|r| (r.name.clone(), r.depth)).collect();
        assert_eq!(names, [("root".into(), 0), ("a".into(), 1), ("x".into(), 2), ("y".into(), 2), ("z".into(), 3), ("b".into(), 1)]);
        // b 는 하위가 없음을 펼친 뒤 안다
        t.toggle(&d.join("b"));
        assert!(!t.visible_rows().iter().find(|r| r.name == "b").unwrap().has_children);
        t.toggle(&d.join("a"));
        assert_eq!(t.visible_rows().len(), 3);
        // 숨김 조건: b 를 숨기면 줄에서 빠진다
        let rows = t.visible_rows_where(&|p| p.ends_with("b"));
        assert!(rows.iter().all(|r| r.name != "b"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
