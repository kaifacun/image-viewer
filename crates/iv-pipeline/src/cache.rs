use std::collections::HashMap;
use std::sync::Arc;

use iv_core::Frame;

use crate::EntryKey;

struct Slot {
    frame: Arc<Frame>,
    last_used: u64,
}

/// 바이트 상한 LRU. 항목 하나에 해상도가 다른 프레임 여러 개(미리보기·화면·원본)를 둔다.
pub struct FrameCache {
    map: HashMap<EntryKey, Vec<Slot>>,
    bytes: usize,
    limit: usize,
    clock: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    pub bytes: usize,
    pub limit: usize,
    pub frames: usize,
}

impl FrameCache {
    pub fn new(limit: usize) -> Self {
        Self { map: HashMap::new(), bytes: 0, limit, clock: 0 }
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats { bytes: self.bytes, limit: self.limit, frames: self.map.values().map(Vec::len).sum() }
    }

    /// 넣기. 같은 크기 프레임이 있으면 교체. 넣은 뒤 상한을 넘으면 오래된 것부터 버린다
    /// (`pinned` 항목은 버리지 않음 — 현재 보고 있는 장).
    pub fn insert(&mut self, key: EntryKey, frame: Arc<Frame>, pinned: &[EntryKey]) {
        self.clock += 1;
        let slots = self.map.entry(key).or_default();
        if let Some(pos) = slots.iter().position(|s| s.frame.max_side() == frame.max_side() && s.frame.is_preview == frame.is_preview) {
            self.bytes -= slots[pos].frame.byte_size();
            slots.remove(pos);
        }
        // 본 디코드가 들어오면 그보다 작은 미리보기는 쓸모없다
        if !frame.is_preview {
            let before: usize = slots.iter().map(|s| s.frame.byte_size()).sum();
            slots.retain(|s| !(s.frame.is_preview && s.frame.max_side() <= frame.max_side()));
            let after: usize = slots.iter().map(|s| s.frame.byte_size()).sum();
            self.bytes -= before - after;
        }
        self.bytes += frame.byte_size();
        slots.push(Slot { frame, last_used: self.clock });
        self.evict(pinned);
    }

    fn evict(&mut self, pinned: &[EntryKey]) {
        while self.bytes > self.limit {
            // 가장 오래 안 쓴 (항목, 슬롯) 찾기
            let victim = self
                .map
                .iter()
                .filter(|(k, _)| !pinned.contains(k))
                .flat_map(|(k, v)| v.iter().enumerate().map(move |(i, s)| (*k, i, s.last_used)))
                .min_by_key(|t| t.2);
            let Some((k, i, _)) = victim else { break };
            let slots = self.map.get_mut(&k).unwrap();
            self.bytes -= slots[i].frame.byte_size();
            slots.remove(i);
            if slots.is_empty() {
                self.map.remove(&k);
            }
        }
    }

    /// 요청 크기를 만족하는 프레임 중 가장 작은 것, 없으면 가장 큰 것.
    /// 두 번째 값은 "요청을 만족하는가".
    pub fn best(&mut self, key: EntryKey, need_side: Option<u32>) -> Option<(Arc<Frame>, bool)> {
        self.clock += 1;
        let clock = self.clock;
        let slots = self.map.get_mut(&key)?;
        let satisfies = |f: &Frame| !f.is_preview && need_side.map_or(f.max_side() >= f.full_size[0].max(f.full_size[1]), |n| f.max_side() >= n || f.max_side() >= f.full_size[0].max(f.full_size[1]));
        let pick = slots
            .iter_mut()
            .filter(|s| satisfies(&s.frame))
            .min_by_key(|s| s.frame.max_side())
            .map(|s| {
                s.last_used = clock;
                (s.frame.clone(), true)
            });
        if pick.is_some() {
            return pick;
        }
        slots.iter_mut().max_by_key(|s| (!s.frame.is_preview, s.frame.max_side())).map(|s| {
            s.last_used = clock;
            (s.frame.clone(), false)
        })
    }

    pub fn has_satisfying(&mut self, key: EntryKey, need_side: Option<u32>) -> bool {
        matches!(self.best(key, need_side), Some((_, true)))
    }

    pub fn contains(&self, key: EntryKey) -> bool {
        self.map.contains_key(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr(side: u32, full: u32, preview: bool) -> Arc<Frame> {
        let mut f = Frame::rgba8(side, side, vec![0; (side * side * 4) as usize], [full, full]);
        f.is_preview = preview;
        Arc::new(f)
    }

    #[test]
    fn best_picks_smallest_satisfying() {
        let mut c = FrameCache::new(usize::MAX);
        c.insert(1, fr(100, 1000, false), &[]);
        c.insert(1, fr(500, 1000, false), &[]);
        c.insert(1, fr(1000, 1000, false), &[]);
        assert_eq!(c.best(1, Some(400)).unwrap().0.max_side(), 500);
        assert_eq!(c.best(1, None).unwrap().0.max_side(), 1000);
        // 원본보다 큰 요구 → 원본이면 만족
        assert!(c.best(1, Some(5000)).unwrap().1);
    }

    #[test]
    fn preview_never_satisfies_and_is_dropped_by_full() {
        let mut c = FrameCache::new(usize::MAX);
        c.insert(1, fr(400, 1000, true), &[]);
        let (f, ok) = c.best(1, Some(300)).unwrap();
        assert!(f.is_preview && !ok);
        c.insert(1, fr(500, 1000, false), &[]);
        assert_eq!(c.stats().frames, 1);
    }

    #[test]
    fn evicts_lru_but_not_pinned() {
        let one = 100 * 100 * 4;
        let mut c = FrameCache::new(one * 2);
        c.insert(1, fr(100, 100, false), &[]);
        c.insert(2, fr(100, 100, false), &[]);
        c.insert(3, fr(100, 100, false), &[1]);
        assert!(c.contains(1) && !c.contains(2) && c.contains(3));
    }
}
