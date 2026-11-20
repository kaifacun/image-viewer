use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// 디코드 우선순위 관문 — 서로 다른 작업 풀(전체 보기 파이프라인 ↔ 썸네일) 사이의 CPU·IO 중재.
///
/// 높은 우선순위 쪽(현재 장 디코드)이 [`Gate::set_busy`] 로 알리고,
/// 낮은 우선순위 쪽(썸네일)은 새 작업을 시작하기 전에 [`Gate::wait_idle`] 로 기다린다.
/// 이미 시작한 낮은 작업은 끝까지 간다 (작은 썸네일이라 짧다).
#[derive(Default, Debug)]
pub struct Gate {
    busy: Mutex<bool>,
    cv: Condvar,
}

impl Gate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_busy(&self, busy: bool) {
        let mut b = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        if *b != busy {
            *b = busy;
            if !busy {
                self.cv.notify_all();
            }
        }
    }

    pub fn is_busy(&self) -> bool {
        *self.busy.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 한가해질 때까지 (최대 `max`) 기다린다. 한가하면 true.
    /// 상한이 있어 높은 쪽 작업이 멈춰도 낮은 쪽이 영원히 굶지 않는다.
    pub fn wait_idle(&self, max: Duration) -> bool {
        let b = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        let (b, _) = self.cv.wait_timeout_while(b, max, |busy| *busy).unwrap_or_else(|e| e.into_inner());
        !*b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn waits_until_idle() {
        let g = Arc::new(Gate::new());
        assert!(g.wait_idle(Duration::from_millis(1)));
        g.set_busy(true);
        assert!(!g.wait_idle(Duration::from_millis(5)));
        let g2 = g.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            g2.set_busy(false);
        });
        let s = Instant::now();
        assert!(g.wait_idle(Duration::from_secs(2)));
        assert!(s.elapsed() < Duration::from_secs(1));
        t.join().unwrap();
    }
}
