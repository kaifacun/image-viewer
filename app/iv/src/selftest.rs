//! 자가 시험 스크립트 (IV_SELFTEST 환경변수) — 실제 창·이벤트 루프·렌더러 경로로 동작을 재현하고
//! 앱 자신의 화면만 PNG 로 저장한다 (화면 전체 캡처 없이 검증).
//!
//! 예: IV_SELFTEST="wait:800;cap:a.png;act:next;wait:200;cap:b.png;act:zoomin;cap:c.png;quit"

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iv_viewer::Action;

#[derive(Debug, Clone)]
pub enum Step {
    Wait(Duration),
    Act(Action),
    Capture(PathBuf),
    Resize(u32, u32),
    /// 렌더러를 강제로 다음 단계로 (장치 손실 모의)
    DeviceLost,
    Quit,
    /// 실입력 주입 (점 좌표) — egui 의 맞음 판정·가려짐까지 거친다
    Click(f32, f32),
    DoubleClick(f32, f32),
    /// 수식키를 누른 채 클릭 (cmd, shift)
    ModClick(f32, f32, bool, bool),
    /// 커서 위치에서 휠 (점, 양수 = 내용이 아래로 → 위쪽이 보임)
    Scroll(f32, f32, f32),
    /// 키 입력 (앱 단축키 표를 거친다): left/right/up/down/enter/escape/backspace/home/end/pageup/pagedown
    Key(String),
    /// 화면에 보이는 썸네일이 모두 채워질 때까지 (최대 ms)
    WaitThumbs(u64),
}

pub struct SelfTest {
    steps: Vec<Step>,
    pos: usize,
    wait_until: Option<Instant>,
}

impl SelfTest {
    pub fn from_env() -> Option<Self> {
        let s = std::env::var("IV_SELFTEST").ok()?;
        let steps = s.split(';').filter(|t| !t.trim().is_empty()).filter_map(|t| parse(t.trim())).collect();
        Some(Self { steps, pos: 0, wait_until: None })
    }

    /// 지금 실행할 다음 단계 (대기 중이면 None 과 깨어날 시각)
    pub fn next(&mut self) -> (Option<Step>, Option<Instant>) {
        if let Some(t) = self.wait_until {
            if Instant::now() < t {
                return (None, Some(t));
            }
            self.wait_until = None;
        }
        let Some(step) = self.steps.get(self.pos).cloned() else { return (None, None) };
        self.pos += 1;
        if let Step::Wait(d) = step {
            let t = Instant::now() + d;
            self.wait_until = Some(t);
            return (None, Some(t));
        }
        (Some(step), Some(Instant::now()))
    }
}

fn parse(t: &str) -> Option<Step> {
    let (k, v) = t.split_once(':').unwrap_or((t, ""));
    Some(match k {
        "wait" => Step::Wait(Duration::from_millis(v.parse().ok()?)),
        "cap" => Step::Capture(PathBuf::from(v)),
        "quit" => Step::Quit,
        "lost" => Step::DeviceLost,
        "resize" => {
            let (w, h) = v.split_once('x')?;
            Step::Resize(w.parse().ok()?, h.parse().ok()?)
        }
        "click" | "dblclick" | "cmdclick" | "shiftclick" => {
            let (x, y) = v.split_once(',')?;
            let (x, y) = (x.trim().parse().ok()?, y.trim().parse().ok()?);
            match k {
                "click" => Step::Click(x, y),
                "dblclick" => Step::DoubleClick(x, y),
                "cmdclick" => Step::ModClick(x, y, true, false),
                _ => Step::ModClick(x, y, false, true),
            }
        }
        "scroll" => {
            let p: Vec<f32> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            let [x, y, dy] = p[..] else { return None };
            Step::Scroll(x, y, dy)
        }
        "key" => Step::Key(v.to_ascii_lowercase()),
        "wait-thumbs" => Step::WaitThumbs(v.parse().unwrap_or(5000)),
        // 동작 이름만 소문자로 (경로 값은 대소문자를 지킨다)
        "act" => Step::Act(match v.split_once('=').map_or(v.to_ascii_lowercase(), |(a, b)| format!("{}={b}", a.to_ascii_lowercase())).as_str() {
            "next" => Action::Next,
            "prev" => Action::Prev,
            "first" => Action::First,
            "last" => Action::Last,
            "zoomin" => Action::ZoomIn,
            "zoomout" => Action::ZoomOut,
            "fit" => Action::Fit,
            "actual" => Action::ActualSize,
            "info" => Action::ToggleInfo,
            "view" => Action::EnterView,
            "browse" => Action::ExitView,
            "parent" => Action::ParentFolder,
            "refresh" => Action::Refresh,
            "selectall" => Action::SelectAll,
            "trash" => Action::Trash,
            other => {
                if let Some(p) = other.strip_prefix("pan=") {
                    let (x, y) = p.split_once(',')?;
                    Action::Pan([x.parse().ok()?, y.parse().ok()?])
                } else if let Some(f) = other.strip_prefix("zoom=") {
                    Action::ZoomBy { factor: f.parse().ok()?, anchor: [400.0, 300.0] }
                } else if let Some(p) = other.strip_prefix("open=") {
                    Action::Open(PathBuf::from(p))
                } else if let Some(p) = other.strip_prefix("folder=") {
                    Action::OpenFolder(PathBuf::from(p))
                } else if let Some(i) = other.strip_prefix("select=") {
                    Action::Select(i.parse().ok()?)
                } else if let Some(i) = other.strip_prefix("toggle=") {
                    Action::ToggleSelect(i.parse().ok()?)
                } else if let Some(i) = other.strip_prefix("range=") {
                    Action::RangeSelect(i.parse().ok()?)
                } else if let Some(i) = other.strip_prefix("activate=") {
                    Action::Activate(i.parse().ok()?)
                } else if let Some(k) = other.strip_prefix("sort=") {
                    let (k, asc) = k.split_once(',').map_or((k, true), |(a, b)| (a, b != "desc"));
                    let key = match k {
                        "date" => iv_viewer::SortKey::Modified,
                        "size" => iv_viewer::SortKey::Size,
                        "type" => iv_viewer::SortKey::Type,
                        _ => iv_viewer::SortKey::Name,
                    };
                    Action::Sort(key, asc)
                } else {
                    log::warn!("자가 시험: 알 수 없는 동작 {other}");
                    return None;
                }
            }
        }),
        _ => return None,
    })
}
