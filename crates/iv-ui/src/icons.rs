//! 선으로 그린 아이콘 버튼 (글꼴에 기호가 없어 네모로 깨지는 문제를 피한다)

use egui::{Color32, CornerRadius, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// 반시계 90°
    RotateLeft,
    /// 시계 90°
    RotateRight,
    /// 경로 복사 (겹친 두 장)
    Copy,
}

/// 아이콘 버튼. `framed` 면 egui 버튼처럼 바탕을 깐다 (도구 막대), 아니면 마우스를 올릴 때만 (아래 막대).
pub fn icon_button(ui: &mut Ui, icon: Icon, tip: &str, enabled: bool, framed: bool) -> Response {
    let size = Vec2::new(26.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let bg = if !enabled {
            if framed { v.widgets.noninteractive.weak_bg_fill } else { Color32::TRANSPARENT }
        } else if resp.is_pointer_button_down_on() {
            v.widgets.active.weak_bg_fill
        } else if resp.hovered() {
            v.widgets.hovered.weak_bg_fill
        } else if framed {
            v.widgets.inactive.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(4), bg);
        let color = if enabled { Color32::from_gray(225) } else { Color32::from_gray(110) };
        paint(ui.painter(), icon, rect.shrink2(Vec2::new(6.0, 4.0)), color);
    }
    resp.on_hover_text(tip)
}

/// 아이콘만 그린다 (메뉴 등에서 쓰려고 분리)
pub fn paint(p: &egui::Painter, icon: Icon, r: Rect, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    match icon {
        Icon::RotateLeft | Icon::RotateRight => {
            let c = r.center();
            let rad = r.height().min(r.width()) * 0.5 - 0.5;
            // 화면 좌표(y 아래)에서 각도가 커지면 시계 방향. 오른쪽 위에서 시작해 아래·왼쪽을 돌아
            // 꼭대기에서 끝나고, 화살촉은 꼭대기에서 오른쪽(시계 방향)을 가리킨다 (흔한 ↻ 모양).
            let (a0, a1) = ((-20f32).to_radians(), 270f32.to_radians());
            let flip = |q: Pos2| if icon == Icon::RotateLeft { Pos2::new(2.0 * c.x - q.x, q.y) } else { q };
            let at = |a: f32| Pos2::new(c.x + rad * a.cos(), c.y + rad * a.sin());
            let pts: Vec<Pos2> = (0..=24).map(|k| flip(at(a0 + (a1 - a0) * k as f32 / 24.0))).collect();
            p.add(Shape::line(pts, stroke));
            let end = at(a1);
            let t = Vec2::new(-a1.sin(), a1.cos()); // 진행 방향
            let n = Vec2::new(a1.cos(), a1.sin()); // 바깥쪽
            let tri = vec![flip(end + t * 3.2), flip(end - t * 1.2 + n * 3.0), flip(end - t * 1.2 - n * 3.0)];
            p.add(Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        Icon::Copy => {
            let w = r.width().min(r.height()) * 0.72;
            let back = Rect::from_min_size(Pos2::new(r.center().x - w * 0.5 - 2.0, r.min.y), Vec2::splat(w));
            let front = back.translate(Vec2::new(4.0, 4.0));
            p.rect_stroke(back, CornerRadius::same(2), stroke, StrokeKind::Middle);
            p.rect_filled(front, CornerRadius::same(2), Color32::from_gray(40));
            p.rect_stroke(front, CornerRadius::same(2), stroke, StrokeKind::Middle);
        }
    }
}
