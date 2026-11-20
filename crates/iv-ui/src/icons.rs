//! 선으로 그린 아이콘 (글꼴에 기호가 없어 네모로 깨지는 문제를 피하고, 크기·굵기를 한 규칙으로 맞춘다)

use egui::{Color32, CornerRadius, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// 반시계 90°
    RotateLeft,
    /// 시계 90°
    RotateRight,
    /// 겹친 두 장 (경로 복사)
    Copy,
    /// 위 화살표 (상위 폴더)
    Up,
    /// 원형 화살표 (새로고침)
    Refresh,
    /// 네 칸 (목록·격자)
    Grid,
    ChevronLeft,
    ChevronRight,
    /// 네 모서리 (전체 보기·맞춤)
    Expand,
    /// 동그라미 i (정보)
    Info,
    Trash,
    /// 오른쪽 칸이 있는 창 (정보 패널)
    Inspector,
    Folder,
    /// 깔때기 (필터)
    Filter,
    /// 저장 (아래 화살표 + 받침)
    Save,
    /// 산 그림 (사진)
    Photo,
    /// 재생 삼각형이 든 네모 (비디오)
    Video,
    Play,
    Pause,
    /// 스피커 + 소리 물결
    Volume,
    /// 스피커 + X
    Mute,
}

/// 아이콘 버튼. `framed` 면 바탕을 깐다 (도구 막대), 아니면 마우스를 올릴 때만 (떠 있는 막대).
pub fn icon_button(ui: &mut Ui, icon: Icon, tip: &str, enabled: bool, framed: bool) -> Response {
    icon_toggle(ui, icon, tip, enabled, framed, false)
}

/// 켜짐 상태가 있는 아이콘 버튼 (켜지면 강조색 바탕)
pub fn icon_toggle(ui: &mut Ui, icon: Icon, tip: &str, enabled: bool, framed: bool, on: bool) -> Response {
    let size = Vec2::new(28.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let bg = if on {
            theme::ACCENT_BG
        } else if !enabled {
            Color32::TRANSPARENT
        } else if resp.is_pointer_button_down_on() {
            theme::PRESSED
        } else if resp.hovered() {
            theme::HOVER
        } else if framed {
            theme::CONTROL
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(6), bg);
        let color = if !enabled {
            theme::TEXT_DISABLED
        } else if on {
            Color32::WHITE
        } else {
            theme::TEXT
        };
        paint(ui.painter(), icon, Rect::from_center_size(rect.center(), Vec2::splat(16.0)), color);
    }
    resp.on_hover_text(tip)
}

/// 아이콘만 그린다 (`r` = 16×16 정도의 상자)
pub fn paint(p: &egui::Painter, icon: Icon, r: Rect, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let c = r.center();
    let at = |x: f32, y: f32| Pos2::new(r.min.x + r.width() * x, r.min.y + r.height() * y);
    let line = |a: Pos2, b: Pos2| p.line_segment([a, b], stroke);
    let arrow_head = |tip: Pos2, dir: Vec2, size: f32| {
        let d = dir.normalized();
        let n = Vec2::new(-d.y, d.x);
        p.add(Shape::convex_polygon(vec![tip + d * 1.0, tip - d * size + n * size * 0.85, tip - d * size - n * size * 0.85], color, Stroke::NONE));
    };
    match icon {
        Icon::RotateLeft | Icon::RotateRight | Icon::Refresh => {
            let rad = r.width().min(r.height()) * 0.40;
            let mirror = icon == Icon::RotateLeft;
            let flip = |q: Pos2| if mirror { Pos2::new(2.0 * c.x - q.x, q.y) } else { q };
            // 오른쪽 위에서 시작해 아래·왼쪽을 돌아 꼭대기에서 끝, 화살촉은 시계 방향
            let (a0, a1) = if icon == Icon::Refresh { ((-30f32).to_radians(), 250f32.to_radians()) } else { ((-20f32).to_radians(), 270f32.to_radians()) };
            let pt = |a: f32| Pos2::new(c.x + rad * a.cos(), c.y + rad * a.sin());
            let pts: Vec<Pos2> = (0..=24).map(|k| flip(pt(a0 + (a1 - a0) * k as f32 / 24.0))).collect();
            p.add(Shape::line(pts, stroke));
            let end = pt(a1);
            let t = Vec2::new(-a1.sin(), a1.cos());
            let t = if mirror { Vec2::new(-t.x, t.y) } else { t };
            arrow_head(flip(end) + t * 1.5, t, 3.2);
        }
        Icon::Copy => {
            let w = r.width() * 0.62;
            let back = Rect::from_min_size(at(0.08, 0.08), Vec2::splat(w));
            let front = back.translate(Vec2::splat(r.width() * 0.28));
            p.rect_stroke(back, CornerRadius::same(2), stroke, StrokeKind::Middle);
            p.rect_filled(front, CornerRadius::same(2), theme::BG_PANEL);
            p.rect_stroke(front, CornerRadius::same(2), stroke, StrokeKind::Middle);
        }
        Icon::Up => {
            line(at(0.5, 0.88), at(0.5, 0.2));
            line(at(0.5, 0.14), at(0.22, 0.42));
            line(at(0.5, 0.14), at(0.78, 0.42));
        }
        Icon::Grid => {
            let s = r.width() * 0.36;
            for (x, y) in [(0.1, 0.1), (0.54, 0.1), (0.1, 0.54), (0.54, 0.54)] {
                p.rect_stroke(Rect::from_min_size(at(x, y), Vec2::splat(s)), CornerRadius::same(1), stroke, StrokeKind::Middle);
            }
        }
        Icon::ChevronLeft => {
            p.add(Shape::line(vec![at(0.62, 0.18), at(0.32, 0.5), at(0.62, 0.82)], Stroke::new(1.8, color)));
        }
        Icon::ChevronRight => {
            p.add(Shape::line(vec![at(0.38, 0.18), at(0.68, 0.5), at(0.38, 0.82)], Stroke::new(1.8, color)));
        }
        Icon::Expand => {
            let k = 0.3;
            for (x, y, dx, dy) in [(0.1, 0.1, 1.0, 1.0), (0.9, 0.1, -1.0, 1.0), (0.1, 0.9, 1.0, -1.0), (0.9, 0.9, -1.0, -1.0)] {
                let o = at(x, y);
                p.add(Shape::line(vec![o + Vec2::new(0.0, dy * r.height() * k), o, o + Vec2::new(dx * r.width() * k, 0.0)], stroke));
            }
        }
        Icon::Info => {
            p.circle_stroke(c, r.width() * 0.42, stroke);
            line(at(0.5, 0.45), at(0.5, 0.74));
            p.circle_filled(at(0.5, 0.3), 1.2, color);
        }
        Icon::Trash => {
            line(at(0.12, 0.24), at(0.88, 0.24));
            p.add(Shape::line(vec![at(0.36, 0.24), at(0.4, 0.1), at(0.6, 0.1), at(0.64, 0.24)], stroke));
            p.add(Shape::line(vec![at(0.22, 0.3), at(0.28, 0.9), at(0.72, 0.9), at(0.78, 0.3)], stroke));
            line(at(0.43, 0.42), at(0.44, 0.76));
            line(at(0.57, 0.42), at(0.56, 0.76));
        }
        Icon::Inspector => {
            let b = Rect::from_min_max(at(0.06, 0.14), at(0.94, 0.86));
            p.rect_stroke(b, CornerRadius::same(2), stroke, StrokeKind::Middle);
            line(at(0.64, 0.14), at(0.64, 0.86));
            line(at(0.72, 0.36), at(0.86, 0.36));
            line(at(0.72, 0.5), at(0.86, 0.5));
        }
        Icon::Folder => {
            p.add(Shape::closed_line(vec![at(0.06, 0.24), at(0.38, 0.24), at(0.48, 0.34), at(0.94, 0.34), at(0.94, 0.84), at(0.06, 0.84)], stroke));
        }
        Icon::Filter => {
            p.add(Shape::closed_line(vec![at(0.1, 0.16), at(0.9, 0.16), at(0.58, 0.52), at(0.58, 0.84), at(0.42, 0.74), at(0.42, 0.52)], stroke));
        }
        Icon::Save => {
            line(at(0.5, 0.1), at(0.5, 0.62));
            p.add(Shape::line(vec![at(0.28, 0.42), at(0.5, 0.64), at(0.72, 0.42)], stroke));
            p.add(Shape::line(vec![at(0.12, 0.66), at(0.12, 0.9), at(0.88, 0.9), at(0.88, 0.66)], stroke));
        }
        Icon::Photo => {
            p.rect_stroke(Rect::from_min_max(at(0.06, 0.16), at(0.94, 0.84)), CornerRadius::same(2), stroke, StrokeKind::Middle);
            p.add(Shape::line(vec![at(0.14, 0.76), at(0.4, 0.46), at(0.58, 0.64), at(0.7, 0.54), at(0.86, 0.72)], stroke));
            p.circle_filled(at(0.7, 0.33), 1.6, color);
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![at(0.24, 0.12), at(0.86, 0.5), at(0.24, 0.88)], color, Stroke::NONE));
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(at(0.22, 0.14), at(0.42, 0.86)), CornerRadius::same(1), color);
            p.rect_filled(Rect::from_min_max(at(0.58, 0.14), at(0.78, 0.86)), CornerRadius::same(1), color);
        }
        Icon::Volume | Icon::Mute => {
            p.add(Shape::convex_polygon(vec![at(0.06, 0.36), at(0.24, 0.36), at(0.46, 0.14), at(0.46, 0.86), at(0.24, 0.64), at(0.06, 0.64)], color, Stroke::NONE));
            if icon == Icon::Mute {
                line(at(0.62, 0.34), at(0.92, 0.66));
                line(at(0.92, 0.34), at(0.62, 0.66));
            } else {
                let c2 = at(0.46, 0.5);
                for rad in [0.22f32, 0.38] {
                    let rr = r.width() * rad;
                    let pts: Vec<Pos2> = (0..=10).map(|k| {
                        let a = (-50.0 + 100.0 * k as f32 / 10.0).to_radians();
                        Pos2::new(c2.x + rr * a.cos(), c2.y + rr * a.sin())
                    }).collect();
                    p.add(Shape::line(pts, stroke));
                }
            }
        }
        Icon::Video => {
            p.rect_stroke(Rect::from_min_max(at(0.06, 0.16), at(0.94, 0.84)), CornerRadius::same(2), stroke, StrokeKind::Middle);
            p.add(Shape::convex_polygon(vec![at(0.4, 0.34), at(0.68, 0.5), at(0.4, 0.66)], color, Stroke::NONE));
        }
    }
}
