//! Paints everything that isn't plain styled text on top of / under the live editor:
//! images, diagrams, math, tables, code block boxes, quote bars, rules and checkboxes.

use std::sync::Arc;

use egui::{
    epaint::text::cursor::CCursor, Align2, Color32, CornerRadius, FontId, Galley, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use pulldown_cmark::Alignment;

use super::doc::{doc_for, ImageRef, RichBlock, RichKind};
use super::layout::{alert_style, base, inline_job, tag_style, Cx, QUOTE_INDENT, RICH_MARGIN};
use super::visual::{self, Env, ImageState, SvgVisual};
use super::PreviewAction;
use crate::theme::colors;

const TABLE_PAD: Vec2 = Vec2::new(10.0, 6.0);
const CELL_SIZE: f32 = 13.5;

pub struct ImgItem {
    pub rect: Rect,
    pub uri: String,
    pub alt: String,
    pub link: Option<String>,
    pub state: u8, // 0 ready, 1 loading, 2 failed
    pub error: String,
}

pub struct TableLayout {
    pub cols: Vec<f32>,
    pub rows: Vec<f32>,
    pub cells: Vec<Vec<Arc<Galley>>>,
    pub size: Vec2,
}

pub enum RichLayout {
    Images { items: Vec<ImgItem>, height: f32 },
    Svg { visual: SvgVisual, size: Vec2 },
    Error { msg: String },
    Table(TableLayout),
    Hidden,
}

impl RichLayout {
    pub fn height(&self) -> f32 {
        match self {
            RichLayout::Images { height, .. } => *height,
            RichLayout::Svg { size, .. } => size.y,
            RichLayout::Error { .. } => 54.0,
            RichLayout::Table(t) => t.size.y,
            RichLayout::Hidden => 0.0,
        }
    }
}

fn fit(size: Vec2, max_w: f32, max_h: f32) -> Vec2 {
    let s = (max_w / size.x).min(max_h / size.y).min(1.0);
    if s.is_finite() && s > 0.0 { size * s } else { size }
}

/// Measures a rich block at the given width. Called by the layouter (to reserve a gap)
/// and by the overlay (to paint), so both agree.
pub fn rich_layout(ui: &Ui, text: &str, block: &RichBlock, width: f32, env: &Env) -> RichLayout {
    let width = if width.is_finite() { width.max(50.0) } else { 800.0 };
    match &block.kind {
        RichKind::Hidden => RichLayout::Hidden,
        RichKind::Mermaid(src) => match visual::mermaid(ui.ctx(), src) {
            Ok(v) => {
                let size = fit(v.size, width, 900.0);
                RichLayout::Svg { visual: v, size }
            }
            Err(e) => RichLayout::Error { msg: format!("Diagram error: {e}") },
        },
        RichKind::Math(src) => match visual::math(ui.ctx(), src, true) {
            Ok(v) => {
                let size = fit(v.size, width, 600.0);
                RichLayout::Svg { visual: v, size }
            }
            Err(e) => RichLayout::Error { msg: format!("Math error: {e}") },
        },
        RichKind::Images(imgs) => images_layout(ui, imgs, width, env),
        RichKind::Table { rows, .. } => RichLayout::Table(table_layout(ui, text, rows, width, env)),
    }
}

fn images_layout(ui: &Ui, imgs: &[ImageRef], width: f32, env: &Env) -> RichLayout {
    let gap = 6.0;
    let (mut x, mut y, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
    let mut items = Vec::new();
    for img in imgs {
        let uri = visual::image_uri(&img.src, env);
        let chip = |label: &str| {
            let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), FontId::proportional(12.0), Color32::WHITE).size().x);
            Vec2::new((w + 20.0).clamp(60.0, width), 26.0)
        };
        let (state, natural, error) = match visual::image_state(ui.ctx(), &uri) {
            ImageState::Ready(s) => (0, s, String::new()),
            ImageState::Loading(Some(s)) => {
                super::layout::mark_volatile();
                (1, s, String::new())
            }
            ImageState::Loading(None) => {
                super::layout::mark_volatile();
                (1, chip(&format!("Loading {}", img.alt)), String::new())
            }
            ImageState::Failed(e) => (2, chip(&format!("⚠ {}", if img.alt.is_empty() { &img.src } else { &img.alt })), e),
        };
        let mut size = natural;
        if let Some(wd) = img.width.filter(|w| *w > 0.0) {
            if state != 2 && size.x > 0.0 {
                size = size * (wd / size.x);
            }
        }
        let size = fit(size.max(Vec2::splat(1.0)), width, 700.0);
        if x > 0.0 && x + size.x > width {
            x = 0.0;
            y += row_h + gap;
            row_h = 0.0;
        }
        items.push(ImgItem {
            rect: Rect::from_min_size(Pos2::new(x, y), size),
            uri,
            alt: img.alt.clone(),
            link: img.link.clone(),
            state,
            error,
        });
        x += size.x + gap;
        row_h = row_h.max(size.y);
    }
    RichLayout::Images { items, height: y + row_h }
}

fn table_layout(ui: &Ui, text: &str, rows: &[Vec<std::ops::Range<usize>>], width: f32, env: &Env) -> TableLayout {
    let doc = doc_for(text);
    let cx = Cx { ui, root: text, doc: &doc, reveal: None, env, wrap: width };
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let fmt_for = |r: usize| {
        let b = base(CELL_SIZE, colors::TEXT_PRIMARY);
        if r == 0 { tag_style("b", &b) } else { b }
    };

    // Natural (unwrapped) column widths.
    let mut natural = vec![0.0f32; ncols];
    for (r, row) in rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let job = inline_job(&cx, &text[cell.clone()], &fmt_for(r), f32::INFINITY);
            let w = ui.fonts_mut(|f| f.layout_job(job)).size().x;
            natural[c] = natural[c].max(w + 2.0 * TABLE_PAD.x);
        }
    }
    let total: f32 = natural.iter().sum();
    let cols: Vec<f32> = if total > width {
        natural.iter().map(|n| (width * n / total).max(48.0)).collect()
    } else {
        natural
    };

    let mut cells = Vec::new();
    let mut heights = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let mut gal = Vec::new();
        let mut h = CELL_SIZE * 1.3;
        for (c, col_w) in cols.iter().enumerate() {
            let src = row.get(c).map_or("", |cell| &text[cell.clone()]);
            let wrap = (col_w - 2.0 * TABLE_PAD.x).max(10.0);
            let g = if src.is_empty() {
                ui.fonts_mut(|f| f.layout_no_wrap(String::new(), FontId::proportional(CELL_SIZE), Color32::WHITE))
            } else {
                let job = inline_job(&cx, src, &fmt_for(r), wrap);
                ui.fonts_mut(|f| f.layout_job(job))
            };
            h = h.max(g.size().y);
            gal.push(g);
        }
        heights.push(h + 2.0 * TABLE_PAD.y);
        cells.push(gal);
    }
    let size = Vec2::new(cols.iter().sum(), heights.iter().sum());
    TableLayout { cols, rows: heights, cells, size }
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

struct Geo<'a> {
    galley: &'a Galley,
    origin: Pos2,
    char_starts: &'a [usize],
}

impl Geo<'_> {
    fn char_idx(&self, byte: usize) -> usize {
        self.char_starts.partition_point(|&b| b < byte)
    }
    /// Screen rect of the row containing `byte` (x is the cursor position there).
    fn row(&self, byte: usize) -> Rect {
        self.galley.pos_from_cursor(CCursor::new(self.char_idx(byte))).translate(self.origin.to_vec2())
    }
}

/// Paints the overlay. `bg_slot` is a shape index reserved before the editor was drawn,
/// so backgrounds end up under the text. Returns a click action (Cmd+click on a linked image).
pub fn paint(
    ui: &mut Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    reveal: Option<(usize, usize)>,
    env: &Env,
    bg_slot: egui::layers::ShapeIdx,
) -> Option<PreviewAction> {
    let doc = doc_for(text);
    let galley = &output.galley;
    let origin = output.galley_pos;
    let width = if galley.job.wrap.max_width.is_finite() { galley.job.wrap.max_width } else { output.response.rect.width() };
    let geo = Geo { galley, origin, char_starts: doc.char_starts(text) };
    let hit = |a: usize, b: usize| reveal.is_some_and(|(x, y)| a <= y && b >= x);
    let left = origin.x;
    let clip = ui.clip_rect();
    let visible = |r: Rect| r.intersects(clip.expand(200.0));

    let mut bg: Vec<Shape> = Vec::new();
    let painter = ui.painter().clone();
    let mut action = None;

    // Frontmatter box
    if let Some(fm) = &doc.frontmatter {
        let r = Rect::from_min_max(Pos2::new(left, geo.row(fm.start).top()), Pos2::new(left + width, geo.row(fm.end).bottom()));
        bg.push(Shape::rect_filled(r, 6.0, Color32::from_rgb(22, 22, 27)));
    }

    // Code blocks
    for cb in &doc.code {
        let top = geo.row(cb.range.start).top();
        let bottom = geo.row(cb.range.end).bottom();
        let r = Rect::from_min_max(Pos2::new(left, top), Pos2::new(left + width, bottom));
        if !visible(r) {
            continue;
        }
        bg.push(Shape::rect_filled(r, 6.0, colors::BG_CODE));
        bg.push(Shape::rect_stroke(r, 6.0, Stroke::new(1.0, colors::LINE), StrokeKind::Inside));
        if cb.fenced && !hit(cb.range.start, cb.range.end) {
            let label = if cb.lang.is_empty() { "code" } else { cb.lang.as_str() };
            painter.text(Pos2::new(left + 14.0, top + 15.0), Align2::LEFT_CENTER, label, FontId::proportional(11.0), colors::TEXT_MUTED);
            let btn = Rect::from_min_size(Pos2::new(left + width - 58.0, top + 4.0), Vec2::new(50.0, 22.0));
            if ui.put(btn, egui::Button::new(RichText::new("Copy").size(10.5).color(colors::TEXT_SECONDARY))).clicked() {
                let block = &text[cb.range.clone()];
                let first_nl = block.find('\n').map_or(block.len(), |i| i + 1);
                let last_nl = block.rfind('\n').unwrap_or(block.len());
                let body = if last_nl > first_nl { &block[first_nl..last_nl] } else { &block[first_nl..] };
                ui.ctx().copy_text(body.to_string());
            }
        }
    }

    // Quotes and alerts
    for q in &doc.quotes {
        let top = geo.row(q.range.start).top();
        let bottom = geo.row(q.range.end).bottom();
        let x = left + QUOTE_INDENT * (q.depth as f32 - 1.0);
        let color = q.alert.map_or(colors::ACCENT, |a| alert_style(a).1);
        let r = Rect::from_min_max(Pos2::new(x, top), Pos2::new(left + width, bottom));
        if !visible(r) {
            continue;
        }
        if q.depth == 1 {
            let tint = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 14);
            bg.push(Shape::rect_filled(r, CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 }, tint));
        }
        bg.push(Shape::rect_filled(Rect::from_min_size(Pos2::new(x, top), Vec2::new(3.0, bottom - top)), 1.5, color));
    }

    // Horizontal rules
    for &start in &doc.rules {
        if doc.rich_at(start).is_some() || doc.code_at(start).is_some() {
            continue;
        }
        let row = geo.row(start);
        let y = row.center().y;
        painter.line_segment([Pos2::new(left, y), Pos2::new(left + width, y)], Stroke::new(1.0, colors::LINE));
    }

    // H1/H2 underline
    for (start, level, _) in &doc.headings {
        if *level <= 2 && doc.quote_depth_at(*start).is_none() {
            // Under the heading's last row (it may wrap); the row has extra space below the text for this.
            let line_end = text[*start..].find('\n').map_or(text.len(), |i| start + i);
            let row = geo.row(line_end);
            if row.height() > 4.0 {
                let y = row.bottom() - 6.0;
                painter.line_segment([Pos2::new(left, y), Pos2::new(left + width, y)], Stroke::new(1.0, colors::LINE));
            }
        }
    }

    // Task checkboxes
    for (range, checked) in &doc.tasks {
        let a = geo.row(range.start);
        let b = geo.row(range.end);
        if !visible(a) || a.top() != b.top() {
            continue;
        }
        let side = 14.0;
        let c = Pos2::new((a.left() + b.left()) / 2.0, a.center().y);
        let r = Rect::from_center_size(c, Vec2::splat(side));
        if *checked {
            painter.rect_filled(r, 3.0, colors::ACCENT);
            let s = Stroke::new(1.8, Color32::WHITE);
            painter.line_segment([Pos2::new(r.left() + 3.2, c.y), Pos2::new(c.x - 1.0, r.bottom() - 3.5)], s);
            painter.line_segment([Pos2::new(c.x - 1.0, r.bottom() - 3.5), Pos2::new(r.right() - 3.0, r.top() + 3.5)], s);
        } else {
            painter.rect_stroke(r, 3.0, Stroke::new(1.3, colors::TEXT_SECONDARY), StrokeKind::Inside);
        }
    }

    // Inline math
    for (&start, &end) in &doc.inline_math {
        if doc.rich_at(start).is_some() || doc.code_at(start).is_some() {
            continue;
        }
        let src = &text[start..end];
        let Ok(v) = visual::math(ui.ctx(), src.trim_matches('$'), src.starts_with("$$")) else { continue };
        let row = geo.row(start);
        if !visible(row) {
            continue;
        }
        let r = Rect::from_min_size(Pos2::new(row.left() - v.size.x, row.center().y - v.size.y / 2.0), v.size);
        egui::Image::new(v.uri.as_str()).paint_at(ui, r);
    }

    // Rich blocks (images, diagrams, display math, tables)
    let pointer = ui.input(|i| (i.pointer.interact_pos(), i.pointer.primary_clicked(), i.modifiers.command));
    for block in &doc.rich {
        let is_table = matches!(block.kind, RichKind::Table { .. });
        if matches!(block.kind, RichKind::Hidden) || (!is_table && hit(block.range.start, block.range.end)) {
            continue;
        }
        let row = geo.row(block.range.start);
        if !visible(row) {
            continue;
        }
        let top_left = Pos2::new(left, row.top() + RICH_MARGIN);
        let lay = rich_layout(ui, text, block, width, env);
        match lay {
            RichLayout::Hidden => {}
            RichLayout::Svg { visual, size } => {
                let r = Rect::from_min_size(Pos2::new(left + (width - size.x).max(0.0) / 2.0, top_left.y), size);
                egui::Image::new(visual.uri.as_str()).paint_at(ui, r);
            }
            RichLayout::Error { msg } => {
                let r = Rect::from_min_size(top_left, Vec2::new(width, 54.0));
                painter.rect_filled(r, 6.0, Color32::from_rgb(40, 22, 24));
                painter.text(r.left_top() + Vec2::new(12.0, 10.0), Align2::LEFT_TOP, msg, FontId::proportional(12.5), colors::DANGER);
                painter.text(r.left_top() + Vec2::new(12.0, 32.0), Align2::LEFT_TOP, "Click to edit the source", FontId::proportional(11.0), colors::TEXT_MUTED);
            }
            RichLayout::Images { items, .. } => {
                for it in items {
                    let r = it.rect.translate(top_left.to_vec2());
                    match it.state {
                        0 => {
                            egui::Image::new(it.uri.as_str()).corner_radius(3).paint_at(ui, r);
                        }
                        1 => {
                            painter.rect_filled(r, 4.0, colors::BG_CARD);
                            painter.text(r.center(), Align2::CENTER_CENTER, format!("Loading {}", it.alt), FontId::proportional(11.5), colors::TEXT_MUTED);
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                        }
                        _ => {
                            painter.rect_stroke(r, 4.0, Stroke::new(1.0, colors::LINE), StrokeKind::Inside);
                            let label = if it.alt.is_empty() { "image" } else { it.alt.as_str() };
                            painter.text(r.center(), Align2::CENTER_CENTER, format!("⚠ {label}"), FontId::proportional(12.0), colors::TEXT_MUTED);
                        }
                    }
                    let hovered = pointer.0.is_some_and(|p| r.contains(p));
                    if hovered && it.state == 2 && !it.error.is_empty() {
                        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::new("img_err"), egui::PopupAnchor::Pointer)
                            .show(|ui| ui.label(&it.error));
                    }
                    if let Some(link) = &it.link {
                        if hovered && pointer.2 {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            if pointer.1 {
                                action = Some(if link.contains("://") { PreviewAction::OpenUrl(link.clone()) } else { PreviewAction::OpenNote(link.clone()) });
                            }
                        }
                    }
                }
            }
            RichLayout::Table(t) => {
                if let Some(a) = table_interactive(ui, text, block, &t, top_left) {
                    action = Some(a);
                }
            }
        }
    }

    painter.set(bg_slot, Shape::Vec(bg));
    action
}

/// Which table cell is being edited: (table start byte, row, column).
type CellKey = (usize, usize, usize);

fn active_cell_id() -> egui::Id {
    egui::Id::new("md_table_active_cell")
}

/// Escapes a cell's text so it stays one cell: no newlines, `|` escaped.
fn escape_cell(s: &str) -> String {
    let mut out = String::new();
    let mut prev = '\0';
    for c in s.chars() {
        match c {
            '\n' | '\r' => out.push(' '),
            '|' if prev != '\\' => out.push_str("\\|"),
            _ => out.push(c),
        }
        prev = c;
    }
    out
}

/// Rebuilds a Markdown table from cell texts.
fn table_source(rows: &[Vec<String>], aligns: &[Alignment]) -> String {
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let line = |cells: &[String]| {
        let mut s = String::from("|");
        for c in 0..ncols {
            s.push(' ');
            s.push_str(cells.get(c).map_or("", |x| x.as_str()));
            s.push_str(" |");
        }
        s
    };
    let mut out = Vec::new();
    if let Some(head) = rows.first() {
        out.push(line(head));
    }
    let mut sep = String::from("|");
    for c in 0..ncols {
        sep.push_str(match aligns.get(c) {
            Some(Alignment::Left) => " :--- |",
            Some(Alignment::Center) => " :---: |",
            Some(Alignment::Right) => " ---: |",
            _ => " --- |",
        });
    }
    out.push(sep);
    for r in rows.iter().skip(1) {
        out.push(line(r));
    }
    out.join("\n")
}

enum TableOp {
    InsertRow(usize),
    DeleteRow(usize),
    InsertCol(usize),
    DeleteCol(usize),
}

/// Draws a table whose cells can be clicked and edited in place.
fn table_interactive(ui: &mut Ui, text: &str, block: &RichBlock, t: &TableLayout, origin: Pos2) -> Option<PreviewAction> {
    let RichKind::Table { aligns, rows } = &block.kind else { return None };
    let painter = ui.painter().clone();
    let table_key = block.range.start;
    let mine = ui.data(|d| d.get_temp::<CellKey>(active_cell_id())).filter(|(k, _, _)| *k == table_key);

    // Frame, header and zebra backgrounds.
    let r = Rect::from_min_size(origin, t.size);
    painter.rect_filled(
        Rect::from_min_size(origin, Vec2::new(t.size.x, t.rows.first().copied().unwrap_or(0.0))),
        CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
        Color32::from_white_alpha(10),
    );

    let ncols = t.cols.len();
    let nrows = t.cells.len();
    let mut result = None;
    let mut new_active: Option<Option<CellKey>> = None;
    let mut op: Option<TableOp> = None;

    let mut y = origin.y;
    for (ri, row) in t.cells.iter().enumerate() {
        let h = t.rows[ri];
        if ri > 0 && ri % 2 == 0 {
            painter.rect_filled(Rect::from_min_size(Pos2::new(origin.x, y), Vec2::new(t.size.x, h)), 0.0, Color32::from_white_alpha(4));
        }
        let mut x = origin.x;
        for (ci, g) in row.iter().enumerate() {
            let cw = t.cols[ci];
            let cell = Rect::from_min_size(Pos2::new(x, y), Vec2::new(cw, h));
            let range = rows.get(ri).and_then(|r| r.get(ci)).cloned();
            let id = egui::Id::new(("md_table_cell", table_key, ri, ci));
            let resp = ui.interact(cell, id.with("click"), Sense::click());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
            let is_active = mine == Some((table_key, ri, ci));

            if is_active {
                if let Some(range) = &range {
                    painter.rect_stroke(cell.shrink(1.0), 3.0, Stroke::new(1.5, colors::ACCENT), StrokeKind::Inside);
                    let has_focus = ui.memory(|m| m.has_focus(id));
                    // Keyboard navigation between cells.
                    let (tab, shift_tab) = if has_focus {
                        ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::Tab), i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab)))
                    } else {
                        (false, false)
                    };
                    let mut buf = text[range.clone()].to_string();
                    let inner = Rect::from_min_max(
                        Pos2::new(cell.left() + TABLE_PAD.x - 2.0, cell.top() + TABLE_PAD.y - 2.0),
                        Pos2::new(cell.right() - TABLE_PAD.x + 2.0, cell.bottom() - TABLE_PAD.y + 2.0),
                    );
                    let te = ui.put(
                        inner,
                        egui::TextEdit::singleline(&mut buf)
                            .id(id)
                            .frame(false)
                            .margin(egui::Margin::same(2))
                            .font(FontId::proportional(CELL_SIZE))
                            .text_color(colors::TEXT_PRIMARY)
                            .desired_width(inner.width()),
                    );
                    if !has_focus && !te.lost_focus() && !te.has_focus() {
                        te.request_focus();
                    }
                    if te.changed() {
                        result = Some(PreviewAction::Replace(range.clone(), escape_cell(&buf)));
                    }
                    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if tab || shift_tab || enter {
                        let next = if enter {
                            Some((ri + 1, ci))
                        } else if tab {
                            if ci + 1 < ncols { Some((ri, ci + 1)) } else { Some((ri + 1, 0)) }
                        } else if ci > 0 {
                            Some((ri, ci - 1))
                        } else if ri > 0 {
                            Some((ri - 1, ncols - 1))
                        } else {
                            None
                        };
                        if let Some((nr, nc)) = next {
                            if nr >= nrows {
                                op = Some(TableOp::InsertRow(nrows));
                            }
                            new_active = Some(Some((table_key, nr, nc)));
                        }
                    } else if te.lost_focus() {
                        new_active = Some(None);
                    }
                }
            } else {
                let inner_w = cw - 2.0 * TABLE_PAD.x;
                let dx = match aligns.get(ci) {
                    Some(Alignment::Center) => (inner_w - g.size().x).max(0.0) / 2.0,
                    Some(Alignment::Right) => (inner_w - g.size().x).max(0.0),
                    _ => 0.0,
                };
                painter.galley(Pos2::new(x + TABLE_PAD.x + dx, y + TABLE_PAD.y), g.clone(), colors::TEXT_PRIMARY);
                if resp.clicked() && range.is_some() {
                    new_active = Some(Some((table_key, ri, ci)));
                }
            }

            resp.context_menu(|ui| {
                ui.set_min_width(170.0);
                if ri > 0 && ui.button("Insert row above").clicked() {
                    op = Some(TableOp::InsertRow(ri));
                    ui.close();
                }
                if ui.button("Insert row below").clicked() {
                    op = Some(TableOp::InsertRow(ri + 1));
                    ui.close();
                }
                if ri > 0 && ui.button("Delete row").clicked() {
                    op = Some(TableOp::DeleteRow(ri));
                    ui.close();
                }
                ui.separator();
                if ui.button("Insert column left").clicked() {
                    op = Some(TableOp::InsertCol(ci));
                    ui.close();
                }
                if ui.button("Insert column right").clicked() {
                    op = Some(TableOp::InsertCol(ci + 1));
                    ui.close();
                }
                if ncols > 1 && ui.button("Delete column").clicked() {
                    op = Some(TableOp::DeleteCol(ci));
                    ui.close();
                }
            });

            x += cw;
            if ci + 1 < row.len() {
                painter.line_segment([Pos2::new(x, y), Pos2::new(x, y + h)], Stroke::new(1.0, colors::LINE));
            }
        }
        y += h;
        if ri + 1 < t.cells.len() {
            painter.line_segment([Pos2::new(origin.x, y), Pos2::new(origin.x + t.size.x, y)], Stroke::new(1.0, colors::LINE));
        }
    }
    painter.rect_stroke(r, 6.0, Stroke::new(1.0, colors::LINE), StrokeKind::Inside);

    // Structural edits rebuild the whole table.
    if let Some(op) = op {
        let mut cells: Vec<Vec<String>> =
            rows.iter().map(|r| (0..ncols).map(|c| r.get(c).map_or(String::new(), |rg| text[rg.clone()].to_string())).collect()).collect();
        let mut al = aligns.clone();
        al.resize(ncols, Alignment::None);
        match op {
            TableOp::InsertRow(at) => cells.insert(at.clamp(1, cells.len()), vec![String::new(); ncols]),
            TableOp::DeleteRow(at) => {
                if at > 0 && at < cells.len() {
                    cells.remove(at);
                }
            }
            TableOp::InsertCol(at) => {
                for row in &mut cells {
                    row.insert(at.min(row.len()), String::new());
                }
                al.insert(at.min(al.len()), Alignment::None);
            }
            TableOp::DeleteCol(at) => {
                for row in &mut cells {
                    if at < row.len() {
                        row.remove(at);
                    }
                }
                if at < al.len() {
                    al.remove(at);
                }
            }
        }
        result = Some(PreviewAction::Replace(block.range.clone(), table_source(&cells, &al)));
    }

    if let Some(a) = new_active {
        ui.data_mut(|d| match a {
            Some(k) => d.insert_temp(active_cell_id(), k),
            None => d.remove::<CellKey>(active_cell_id()),
        });
        ui.ctx().request_repaint();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_tables_and_escapes_cells() {
        let rows = vec![vec!["A".to_string(), "B".to_string()], vec!["1".to_string(), String::new()]];
        let src = table_source(&rows, &[Alignment::Left, Alignment::Right]);
        assert_eq!(src, "| A | B |\n| :--- | ---: |\n| 1 |  |");
        // The rebuilt text parses back into the same table.
        let d = super::super::doc::doc_for(&src);
        assert!(matches!(&d.rich[0].kind, RichKind::Table { rows, .. } if rows.len() == 2));
        assert_eq!(escape_cell("a|b\nc \\| d"), "a\\|b c \\| d");
    }
}
