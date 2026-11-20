//! 마크다운 보기 — pulldown-cmark 로 해석하고 egui 로 그린다 (KaiIV 테마).
//! 제목·문단(굵게·기울임·취소선·인라인 코드·링크)·목록(중첩·번호·할 일)·인용·코드 블록·표·구분선.
//! 그림은 대체 글로 보인다. 글자는 선택·복사할 수 있다.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, Ui};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme;

const BODY: f32 = 14.5;
const CODE_COLOR: Color32 = Color32::from_rgb(240, 160, 120);
const LINK: Color32 = Color32::from_rgb(110, 170, 255);

#[derive(Default)]
struct Style {
    strong: u32,
    emph: u32,
    strike: u32,
    link: u32,
    heading: Option<HeadingLevel>,
}

/// 쌓아 두는 블록 상태
enum Block {
    /// 글 줄 (문단·제목·목록 항목 글)
    Text(LayoutJob),
    Code(String),
    Table { rows: Vec<Vec<String>>, cell: String },
}

pub fn show(ui: &mut Ui, src: &str) {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut st = Style::default();
    let mut block: Option<Block> = None;
    // 목록: 깊이마다 다음 번호 (None = 글머리표)
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote = 0usize;
    let mut pending_marker: Option<String> = None;

    let indent = |lists: &Vec<Option<u64>>, quote: usize| lists.len() as f32 * 22.0 + quote as f32 * 16.0;

    for ev in Parser::new_ext(src, opts) {
        match ev {
            //#region 블록 시작·끝
            Event::Start(Tag::Paragraph) | Event::Start(Tag::Heading { .. }) => {
                if let Event::Start(Tag::Heading { level, .. }) = &ev {
                    st.heading = Some(*level);
                    ui.add_space(if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) { 14.0 } else { 8.0 });
                }
                if !matches!(block, Some(Block::Text(_))) {
                    block = Some(Block::Text(LayoutJob::default()));
                }
            }
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Heading(_)) | Event::End(TagEnd::Item) => {
                let h = st.heading.take();
                if let Some(Block::Text(job)) = block.take() {
                    flush_text(ui, job, pending_marker.take(), indent(&lists, quote), quote > 0);
                }
                if matches!(h, Some(HeadingLevel::H1 | HeadingLevel::H2)) {
                    let r = ui.max_rect();
                    let y = ui.cursor().min.y + 2.0;
                    ui.painter().hline(r.x_range(), y, Stroke::new(1.0, theme::BORDER));
                    ui.add_space(6.0);
                }
                ui.add_space(if lists.is_empty() { 6.0 } else { 2.0 });
            }
            Event::Start(Tag::List(start)) => {
                // 항목 글이 남아 있으면 먼저 (중첩 목록 앞)
                if let Some(Block::Text(job)) = block.take() {
                    flush_text(ui, job, pending_marker.take(), indent(&lists, quote), quote > 0);
                }
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                if lists.is_empty() {
                    ui.add_space(6.0);
                }
            }
            Event::Start(Tag::Item) => {
                if let Some(Block::Text(job)) = block.take() {
                    flush_text(ui, job, pending_marker.take(), indent(&lists, quote), quote > 0);
                }
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => if lists.len() > 1 { "◦" } else { "•" }.to_string(),
                };
                pending_marker = Some(marker);
                block = Some(Block::Text(LayoutJob::default()));
            }
            Event::TaskListMarker(done) => pending_marker = Some(if done { "☑".into() } else { "☐".into() }),
            Event::Start(Tag::BlockQuote(_)) => quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote = quote.saturating_sub(1),
            Event::Start(Tag::CodeBlock(kind)) => {
                let _ = matches!(kind, CodeBlockKind::Fenced(_));
                block = Some(Block::Code(String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(Block::Code(code)) = block.take() {
                    let left = indent(&lists, quote);
                    ui.horizontal(|ui| {
                        ui.add_space(left);
                        egui::Frame::new().fill(theme::BG_PANEL).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(10)).show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.add(egui::Label::new(RichText::new(code.trim_end()).font(FontId::monospace(12.5)).color(theme::TEXT)).selectable(true));
                        });
                    });
                    ui.add_space(8.0);
                }
            }
            Event::Start(Tag::Table(_)) => block = Some(Block::Table { rows: Vec::new(), cell: String::new() }),
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                if let Some(Block::Table { rows, .. }) = &mut block {
                    rows.push(Vec::new());
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(Block::Table { rows, cell }) = &mut block {
                    if let Some(r) = rows.last_mut() {
                        r.push(std::mem::take(cell));
                    }
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(Block::Table { rows, .. }) = block.take() {
                    draw_table(ui, &rows);
                    ui.add_space(8.0);
                }
            }
            Event::Rule => {
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
            }
            //#endregion
            //#region 글자 모양
            Event::Start(Tag::Strong) => st.strong += 1,
            Event::End(TagEnd::Strong) => st.strong = st.strong.saturating_sub(1),
            Event::Start(Tag::Emphasis) => st.emph += 1,
            Event::End(TagEnd::Emphasis) => st.emph = st.emph.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => st.strike += 1,
            Event::End(TagEnd::Strikethrough) => st.strike = st.strike.saturating_sub(1),
            Event::Start(Tag::Link { .. }) => st.link += 1,
            Event::End(TagEnd::Link) => st.link = st.link.saturating_sub(1),
            Event::Start(Tag::Image { .. }) => push(&mut block, "[그림: ", &st, false),
            Event::End(TagEnd::Image) => push(&mut block, "]", &st, false),
            //#endregion
            Event::Text(t) => push(&mut block, &t, &st, false),
            Event::Code(t) => push(&mut block, &t, &st, true),
            Event::SoftBreak => push(&mut block, " ", &st, false),
            Event::HardBreak => push(&mut block, "\n", &st, false),
            Event::Html(t) | Event::InlineHtml(t) => push(&mut block, &t, &st, true),
            _ => {}
        }
    }
    if let Some(Block::Text(job)) = block.take() {
        flush_text(ui, job, pending_marker.take(), 0.0, false);
    }
}

/// 글 조각을 지금 블록에 덧붙인다 (코드 블록·표 칸이면 글 그대로)
fn push(block: &mut Option<Block>, text: &str, st: &Style, code: bool) {
    match block {
        Some(Block::Code(s)) => s.push_str(text),
        Some(Block::Table { cell, .. }) => cell.push_str(text),
        Some(Block::Text(job)) => append(job, text, st, code),
        None => {
            let mut job = LayoutJob::default();
            append(&mut job, text, st, code);
            *block = Some(Block::Text(job));
        }
    }
}

fn append(job: &mut LayoutJob, text: &str, st: &Style, code: bool) {
    let size = match st.heading {
        Some(HeadingLevel::H1) => 26.0,
        Some(HeadingLevel::H2) => 21.0,
        Some(HeadingLevel::H3) => 18.0,
        Some(_) => 16.0,
        None => BODY,
    };
    let mut f = TextFormat { font_id: FontId::new(if code { size * 0.9 } else { size }, if code { FontFamily::Monospace } else { FontFamily::Proportional }), color: theme::TEXT, ..Default::default() };
    if st.heading.is_some() || st.strong > 0 {
        f.color = Color32::WHITE;
    }
    if st.emph > 0 {
        f.italics = true;
    }
    if st.strike > 0 {
        f.strikethrough = Stroke::new(1.0, theme::TEXT_DIM);
        f.color = theme::TEXT_DIM;
    }
    if st.link > 0 {
        f.color = LINK;
        f.underline = Stroke::new(1.0, LINK);
    }
    if code {
        f.color = CODE_COLOR;
        f.background = theme::BG_PANEL;
    }
    job.append(text, 0.0, f);
}

fn flush_text(ui: &mut Ui, mut job: LayoutJob, marker: Option<String>, left: f32, quoted: bool) {
    if job.text.is_empty() && marker.is_none() {
        return;
    }
    job.wrap.max_width = (ui.available_width() - left - 28.0).max(80.0);
    ui.horizontal_top(|ui| {
        ui.add_space(left);
        if quoted {
            let (r, _) = ui.allocate_exact_size(egui::vec2(3.0, 18.0), egui::Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(1), theme::TEXT_FAINT);
            ui.add_space(8.0);
        }
        if let Some(m) = marker {
            ui.label(RichText::new(m).size(BODY).color(theme::TEXT_DIM));
        }
        ui.add(egui::Label::new(job).selectable(true).wrap());
    });
}

fn draw_table(ui: &mut Ui, rows: &[Vec<String>]) {
    egui::Frame::new().stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(8)).show(ui, |ui| {
        egui::Grid::new(ui.next_auto_id()).striped(true).spacing([18.0, 6.0]).show(ui, |ui| {
            for (i, r) in rows.iter().enumerate() {
                for c in r {
                    let t = RichText::new(c).size(13.5);
                    ui.add(egui::Label::new(if i == 0 { t.strong().color(Color32::WHITE) } else { t.color(theme::TEXT) }).selectable(true));
                }
                ui.end_row();
            }
        });
    });
}
