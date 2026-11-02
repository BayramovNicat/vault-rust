//! The live-preview layouter: styles the note like rendered Markdown inside one editable
//! text box. Syntax markers are hidden (drawn 1px and transparent) so every source character
//! is still in the layout and the cursor maps 1:1 onto the text. Rich blocks (images,
//! diagrams, tables, math) are hidden and a gap of the right height is reserved for them;
//! `overlay.rs` paints into those gaps.

use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{
    text::{LayoutJob, TextFormat},
    Color32, FontId, Stroke, Ui,
};

use super::doc::{doc_for, is_fence_line, Alert, Doc, RichKind};
use super::overlay::rich_layout;
use super::visual::{self, Env};
use crate::theme::colors;

pub const BODY_SIZE: f32 = 14.5;
pub const CODE_PAD: f32 = 14.0;
pub const QUOTE_INDENT: f32 = 18.0;
pub const RICH_MARGIN: f32 = 8.0;
/// Background of inline `code`.
const CODE_BG: Color32 = Color32::from_rgb(38, 38, 46);

// ---------------------------------------------------------------------------
// Formats
// ---------------------------------------------------------------------------

pub fn hidden() -> TextFormat {
    TextFormat {
        font_id: FontId::proportional(1.0),
        color: Color32::TRANSPARENT,
        ..Default::default()
    }
}

/// Hidden text whose row is exactly `h` tall (0 collapses the row).
fn hidden_h(h: f32) -> TextFormat {
    TextFormat { line_height: Some(h), ..hidden() }
}

pub fn base(size: f32, color: Color32) -> TextFormat {
    TextFormat {
        font_id: FontId::proportional(size),
        color,
        ..Default::default()
    }
}

fn mono(size: f32, color: Color32) -> TextFormat {
    TextFormat {
        font_id: FontId::monospace(size),
        color,
        ..Default::default()
    }
}

fn link_format(fmt: &TextFormat) -> TextFormat {
    TextFormat {
        color: colors::TEXT_ACCENT,
        underline: Stroke::new(1.0, Color32::from_rgba_unmultiplied(115, 160, 255, 90)),
        ..fmt.clone()
    }
}

pub fn alert_style(a: Alert) -> (&'static str, Color32) {
    match a {
        Alert::Note => ("Note", Color32::from_rgb(88, 166, 255)),
        Alert::Tip => ("Tip", Color32::from_rgb(63, 185, 80)),
        Alert::Important => ("Important", Color32::from_rgb(171, 125, 248)),
        Alert::Warning => ("Warning", Color32::from_rgb(210, 153, 34)),
        Alert::Caution => ("Caution", Color32::from_rgb(248, 81, 73)),
    }
}

// ---------------------------------------------------------------------------
// Context + writer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct Cx<'a> {
    pub ui: &'a Ui,
    pub root: &'a str,
    pub doc: &'a Doc,
    pub reveal: Option<(usize, usize)>,
    pub env: &'a Env,
    pub wrap: f32,
}

impl Cx<'_> {
    /// Absolute byte offset of `sub` (a slice of the note) in the note.
    pub fn off(&self, sub: &str) -> usize {
        sub.as_ptr() as usize - self.root.as_ptr() as usize
    }
    /// Does the cursor/selection touch the absolute byte range `a..=b`?
    pub fn hit(&self, a: usize, b: usize) -> bool {
        self.reveal.is_some_and(|(x, y)| a <= y && b >= x)
    }
    fn glyph_or(&self, c: &'static str, fallback: &'static str) -> &'static str {
        let ch = c.chars().next().unwrap_or(' ');
        if self.ui.fonts_mut(|f| f.has_glyph(&FontId::proportional(BODY_SIZE), ch)) { c } else { fallback }
    }
}

/// Appends sections; the first non-empty one gets the pending leading space (indent).
pub struct W<'j> {
    pub job: &'j mut LayoutJob,
    pub lead: f32,
}

impl W<'_> {
    pub fn add(&mut self, s: &str, fmt: TextFormat) {
        if s.is_empty() {
            return;
        }
        let lead = std::mem::take(&mut self.lead);
        self.job.append(s, lead, fmt);
    }
}

// ---------------------------------------------------------------------------
// Layouter
// ---------------------------------------------------------------------------

enum Mode {
    /// Hidden line with zero height.
    Collapsed,
    /// First line of a rendered rich block: hidden, with a row of this height.
    Gap(f32),
    /// A rich block the cursor is in: show its source as plain monospace.
    Raw,
    Code { fence: bool, opening: bool, active: bool },
    Frontmatter { fence: bool },
    Rule,
    Normal,
}

/// Everything the layout job depends on. Equal keys give equal jobs.
#[derive(PartialEq)]
struct JobKey {
    text: u64,
    wrap: u32,
    pixels_per_point: u32,
    env: u64,
    /// Rich blocks shown as source and code blocks shown with fences (both follow the cursor).
    revealed: Vec<usize>,
}

thread_local! {
    /// The last job built. The editor asks for a layout every frame (cursor blink, hover, scroll),
    /// while the job only changes when the text, width or revealed blocks do.
    static JOB_CACHE: RefCell<Option<(JobKey, LayoutJob)>> = const { RefCell::new(None) };
    /// Set while building a job whose content isn't final (an image still loading).
    static VOLATILE: Cell<bool> = const { Cell::new(false) };
}

/// Marks the job being built as not cacheable.
pub fn mark_volatile() {
    VOLATILE.with(|v| v.set(true));
}

fn job_key(cx: &Cx, doc: &Doc) -> JobKey {
    let mut env = std::collections::hash_map::DefaultHasher::new();
    cx.env.note_dir.hash(&mut env);
    cx.env.vault_root.hash(&mut env);
    let rich = doc.rich.iter().enumerate().filter(|(_, b)| !matches!(b.kind, RichKind::Table { .. }) && cx.hit(b.range.start, b.range.end));
    let code = doc.code.iter().enumerate().filter(|(_, b)| cx.hit(b.range.start, b.range.end));
    // Code blocks go after rich blocks in the list so the two can't be confused.
    let revealed = rich.map(|(i, _)| i).chain(code.map(|(i, _)| doc.rich.len() + i)).collect();
    JobKey {
        text: doc.hash,
        wrap: cx.wrap.to_bits(),
        pixels_per_point: cx.ui.ctx().pixels_per_point().to_bits(),
        env: env.finish(),
        revealed,
    }
}

/// Lays out the note. `reveal` is the byte range touched by the cursor/selection
/// (`None` when the editor isn't focused). Only rich blocks react to it.
pub fn live_layouter(ui: &Ui, text: &str, wrap_width: f32, reveal: Option<(usize, usize)>, env: &Env) -> Arc<egui::Galley> {
    let doc = doc_for(text);
    let cx = Cx { ui, root: text, doc: &doc, reveal, env, wrap: wrap_width };

    let key = job_key(&cx, &doc);
    let cached = JOB_CACHE.with(|c| c.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, job)| job.clone()));
    // egui caches galleys by job, so handing it an equal job is cheap.
    let job = cached.unwrap_or_else(|| {
        VOLATILE.with(|v| v.set(false));
        let job = build_job(&cx, text, wrap_width);
        if !VOLATILE.with(Cell::get) {
            JOB_CACHE.with(|c| *c.borrow_mut() = Some((key, job.clone())));
        }
        job
    });
    ui.fonts_mut(|f| f.layout_job(job))
}

fn build_job(cx: &Cx, text: &str, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;

    // Split into lines and decide how each one is drawn.
    let mut lines: Vec<(usize, usize, Mode)> = Vec::new();
    let mut start = 0;
    loop {
        let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        lines.push((start, end, line_mode(cx, start, end)));
        if end >= text.len() {
            break;
        }
        start = end + 1;
    }

    for (i, (start, end, mode)) in lines.iter().enumerate() {
        let mut w = W { job: &mut job, lead: 0.0 };
        layout_line(cx, &mut w, &text[*start..*end], *start, mode);

        if let Some((_, _, next_mode)) = lines.get(i + 1) {
            // How epaint sizes rows: an empty line takes its height from the format of the
            // newline that *ends* it, and that newline also sets the height of the next line if
            // that one is empty. So an empty line's newline carries its own format; otherwise it
            // carries the next line's format.
            let fmt = if start == end { row_format(mode) } else { row_format(next_mode) };
            job.append("\n", 0.0, fmt);
        }
    }
    job
}

fn line_mode(cx: &Cx, start: usize, end: usize) -> Mode {
    let doc = cx.doc;
    if let Some(rb) = doc.rich_at(start) {
        // Tables are edited cell by cell in the overlay, so they never switch to raw source here.
        if cx.hit(rb.range.start, rb.range.end) && !matches!(rb.kind, RichKind::Table { .. }) {
            return Mode::Raw;
        }
        if start != rb.range.start || matches!(rb.kind, RichKind::Hidden) {
            return Mode::Collapsed;
        }
        let h = rich_layout(cx.ui, cx.root, rb, cx.wrap, cx.env).height();
        return Mode::Gap(h + 2.0 * RICH_MARGIN);
    }
    if let Some(cb) = doc.code_at(start) {
        let first = start == cb.range.start;
        let last = end == cb.range.end;
        let fence = cb.fenced && (first || (last && is_fence_line(&cx.root[start..end])));
        return Mode::Code { fence, opening: first, active: cx.hit(cb.range.start, cb.range.end) };
    }
    if let Some(fm) = &doc.frontmatter {
        if fm.start <= start && start <= fm.end {
            return Mode::Frontmatter { fence: start == fm.start || end == fm.end };
        }
    }
    if doc.setext_underlines.contains(&start) {
        return Mode::Collapsed;
    }
    if doc.rules.contains(&start) {
        return Mode::Rule;
    }
    Mode::Normal
}

fn row_format(mode: &Mode) -> TextFormat {
    match mode {
        Mode::Collapsed => hidden_h(0.0),
        Mode::Gap(h) => hidden_h(*h),
        Mode::Raw => mono(13.0, colors::TEXT_SECONDARY),
        Mode::Code { fence: true, active: false, opening, .. } => hidden_h(if *opening { 30.0 } else { 12.0 }),
        Mode::Code { .. } => mono(13.0, colors::TEXT_PRIMARY),
        Mode::Frontmatter { fence: true } => hidden_h(8.0),
        Mode::Frontmatter { .. } => mono(12.0, colors::TEXT_MUTED),
        Mode::Rule => hidden_h(24.0),
        Mode::Normal => base(BODY_SIZE, colors::TEXT_PRIMARY),
    }
}

fn layout_line(cx: &Cx, w: &mut W, line: &str, start: usize, mode: &Mode) {
    match mode {
        Mode::Collapsed | Mode::Gap(_) | Mode::Rule => w.add(line, row_format(mode)),
        Mode::Raw => w.add(line, mono(13.0, colors::TEXT_SECONDARY)),
        Mode::Frontmatter { fence: true } => w.add(line, hidden_h(8.0)),
        Mode::Frontmatter { .. } => {
            w.lead = CODE_PAD;
            w.add(line, mono(12.0, colors::TEXT_MUTED));
        }
        Mode::Code { fence: true, active, opening } => {
            if *active {
                w.lead = CODE_PAD;
                w.add(line, mono(11.0, Color32::from_rgba_unmultiplied(120, 122, 138, 150)));
            } else {
                w.add(line, hidden_h(if *opening { 30.0 } else { 12.0 }));
            }
        }
        Mode::Code { .. } => code_line(cx, w, line, start),
        Mode::Normal => normal_line(cx, w, line, start),
    }
}

fn code_line(cx: &Cx, w: &mut W, line: &str, start: usize) {
    w.lead = CODE_PAD;
    let plain = mono(13.0, Color32::from_rgb(220, 222, 235));
    let Some(cb) = cx.doc.code_at(start) else {
        w.add(line, plain);
        return;
    };
    let spans = cb.body_line(start).and_then(|i| cb.highlight(cx.root)?.get(i));
    match spans {
        Some(spans) if !spans.is_empty() => {
            let mut at = 0;
            for &(ref r, color, italic) in spans {
                if r.start > at && at < line.len() {
                    w.add(&line[at..r.start.min(line.len())], plain.clone());
                }
                if r.end <= line.len() && r.start <= r.end {
                    w.add(&line[r.clone()], TextFormat { color, italics: italic, ..plain.clone() });
                    at = r.end;
                }
            }
            if at < line.len() {
                w.add(&line[at..], plain);
            }
        }
        _ => w.add(line, plain),
    }
}

fn normal_line(cx: &Cx, w: &mut W, line: &str, start: usize) {
    let body = base(BODY_SIZE, colors::TEXT_PRIMARY);
    let mut rest = line;

    // Blockquote prefix(es): `> > text` -> hidden, indented.
    let mut fmt = body.clone();
    if let Some(q) = cx.doc.quote_depth_at(start) {
        let mut depth = 0;
        loop {
            let t = rest.trim_start_matches(' ');
            if rest.len() - t.len() > 3 {
                break;
            }
            let Some(after) = t.strip_prefix('>') else { break };
            let after = after.strip_prefix(' ').unwrap_or(after);
            depth += 1;
            let consumed = rest.len() - after.len();
            w.add(&rest[..consumed], hidden());
            rest = after;
        }
        w.lead = QUOTE_INDENT * depth.max(q.depth) as f32 + 4.0;
        if let Some(alert) = cx.doc.alert_at(start) {
            let t = rest.trim();
            if t.starts_with("[!") && t.ends_with(']') && t.len() > 3 {
                let (title, color) = alert_style(alert);
                let i = rest.find("[!").unwrap_or(0);
                let close = rest.rfind(']').unwrap_or(rest.len() - 1);
                let label = &rest[i + 2..close];
                w.add(&rest[..i + 2], hidden());
                // Same char count: show the alert title in place of its keyword.
                let shown: String = title.chars().chain(std::iter::repeat(' ')).take(label.chars().count()).collect();
                w.add(&shown, TextFormat { color, ..base(BODY_SIZE, color) });
                w.add(&rest[close..], hidden());
                return;
            }
            fmt = body.clone();
        } else {
            fmt = TextFormat { italics: true, ..base(BODY_SIZE, colors::TEXT_SECONDARY) };
        }
    }

    // Footnote definition: `[^1]: text`
    if cx.doc.in_footnote_def(start) {
        if let Some(r) = rest.strip_prefix("[^") {
            if let Some(close) = r.find("]:") {
                w.add("[^", hidden());
                w.add(&r[..close], base(12.0, colors::TEXT_ACCENT));
                w.add("]:", base(12.0, colors::TEXT_MUTED));
                inline(cx, w, &r[close + 2..], &base(13.0, colors::TEXT_SECONDARY));
                return;
            }
        }
        fmt = base(13.0, colors::TEXT_SECONDARY);
    }

    // Setext heading title line.
    if let Some(&level) = cx.doc.setext_titles.get(&start) {
        inline(cx, w, rest, &heading_format(level as usize));
        return;
    }

    body_line(cx, w, rest, &fmt);
}

/// Text format for a heading. H1/H2 rows get extra space below the text for their underline.
fn heading_format(level: usize) -> TextFormat {
    let (size, color) = heading_style(level);
    let mut f = base(size, color);
    if level <= 2 {
        f.line_height = Some((size * 1.25 + 14.0).round());
        f.valign = egui::Align::TOP;
    }
    f
}

pub fn heading_style(level: usize) -> (f32, Color32) {
    match level {
        1 => (28.0, Color32::WHITE),
        2 => (22.0, Color32::from_rgb(240, 240, 248)),
        3 => (18.0, Color32::from_rgb(232, 232, 242)),
        4 => (16.0, Color32::from_rgb(220, 220, 232)),
        5 => (14.5, colors::TEXT_SECONDARY),
        _ => (13.5, colors::TEXT_MUTED),
    }
}

/// Returns `(checked, byte range of "[ ]")` relative to `rest` for `- [ ] ` style lines.
pub fn task_prefix(rest: &str) -> Option<(bool, std::ops::Range<usize>)> {
    let b = rest.as_bytes();
    let marker = if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && b[1] == b' ' {
        2
    } else {
        let d = b.iter().take_while(|c| c.is_ascii_digit()).count();
        if d > 0 && b.len() > d + 1 && matches!(b[d], b'.' | b')') && b[d + 1] == b' ' { d + 2 } else { return None }
    };
    if b.len() >= marker + 4 && b[marker] == b'[' && b[marker + 2] == b']' && (b.len() == marker + 3 || b[marker + 3] == b' ') {
        match b[marker + 1] {
            b' ' => Some((false, marker..marker + 3)),
            b'x' | b'X' => Some((true, marker..marker + 3)),
            _ => None,
        }
    } else {
        None
    }
}

/// Headings, lists, tasks, definition lists and paragraphs.
fn body_line(cx: &Cx, w: &mut W, line: &str, fmt: &TextFormat) {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let indent_cols: usize = indent.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum();
    if indent_len > 0 {
        w.add(indent, hidden());
        w.lead += indent_cols as f32 * 6.0;
    }

    // ATX heading
    let hashes = rest.bytes().take_while(|&b| b == b'#').count();
    if indent_cols < 4 && (1..=6).contains(&hashes) && (rest.len() == hashes || rest.as_bytes()[hashes] == b' ') {
        let marker_len = (hashes + 1).min(rest.len());
        w.add(&rest[..marker_len], hidden());
        let body = rest[marker_len..].trim_end_matches(['#', ' ']);
        inline(cx, w, body, &heading_format(hashes));
        w.add(&rest[marker_len + body.len()..], hidden());
        return;
    }

    // Task item
    if let Some((checked, r)) = task_prefix(rest) {
        w.add(&rest[..r.start], hidden());
        w.lead += 2.0;
        // The box itself is painted by the overlay over these transparent chars.
        w.add(&rest[r.clone()], mono(BODY_SIZE, Color32::TRANSPARENT));
        let item_fmt = if checked {
            TextFormat { strikethrough: Stroke::new(1.0, colors::TEXT_MUTED), ..base(fmt.font_id.size, colors::TEXT_MUTED) }
        } else {
            fmt.clone()
        };
        inline(cx, w, &rest[r.end..], &item_fmt);
        return;
    }

    let b = rest.as_bytes();
    // Bullet
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && (b[1] == b' ' || b[1] == b'\t') {
        let level = indent_cols / 2;
        let glyph = match level % 3 {
            0 => "•",
            1 => cx.glyph_or("◦", "•"),
            _ => cx.glyph_or("▪", "•"),
        };
        w.lead += 4.0;
        w.add(glyph, base(BODY_SIZE, colors::TEXT_MUTED));
        w.add(&rest[1..2], fmt.clone());
        inline(cx, w, &rest[2..], fmt);
        return;
    }
    // Ordered list
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits < 10 && b.len() > digits + 1 && matches!(b[digits], b'.' | b')') && b[digits + 1] == b' ' {
        w.lead += 2.0;
        w.add(&rest[..digits + 1], base(fmt.font_id.size, colors::TEXT_SECONDARY));
        w.add(" ", fmt.clone());
        inline(cx, w, &rest[digits + 2..], fmt);
        return;
    }
    // Definition list: `: definition`
    if let Some(def) = rest.strip_prefix(": ") {
        w.add(": ", hidden());
        w.lead += 24.0;
        inline(cx, w, def, &TextFormat { color: colors::TEXT_SECONDARY, ..fmt.clone() });
        return;
    }

    inline(cx, w, rest, fmt);
}

// ---------------------------------------------------------------------------
// Inline formatting
// ---------------------------------------------------------------------------

const PAIRED_TAGS: [&str; 15] = ["kbd", "mark", "u", "ins", "sub", "sup", "b", "strong", "i", "em", "s", "del", "strike", "code", "small"];

pub fn tag_style(name: &str, fmt: &TextFormat) -> TextFormat {
    let size = fmt.font_id.size;
    match name {
        "kbd" => TextFormat {
            font_id: FontId::monospace(size * 0.85),
            background: Color32::from_rgb(40, 42, 50),
            color: colors::TEXT_PRIMARY,
            ..fmt.clone()
        },
        "code" => TextFormat {
            font_id: FontId::monospace(size * 0.88),
            color: Color32::from_rgb(240, 180, 140),
            background: CODE_BG,
            expand_bg: 1.5,
            ..fmt.clone()
        },
        "mark" => TextFormat { background: Color32::from_rgba_unmultiplied(200, 165, 40, 70), color: Color32::WHITE, ..fmt.clone() },
        "u" | "ins" => TextFormat { underline: Stroke::new(1.0, fmt.color), ..fmt.clone() },
        "sub" => TextFormat { font_id: FontId::proportional(size * 0.72), valign: egui::Align::BOTTOM, ..fmt.clone() },
        "sup" => TextFormat { font_id: FontId::proportional(size * 0.72), valign: egui::Align::TOP, ..fmt.clone() },
        "b" | "strong" => TextFormat { color: Color32::WHITE, font_id: FontId::proportional(size + 0.5), ..fmt.clone() },
        "i" | "em" => TextFormat { italics: true, ..fmt.clone() },
        "s" | "del" | "strike" => TextFormat { strikethrough: Stroke::new(1.0, fmt.color), color: colors::TEXT_SECONDARY, ..fmt.clone() },
        "small" => TextFormat { font_id: FontId::proportional(size * 0.82), ..fmt.clone() },
        _ => fmt.clone(),
    }
}

/// Builds a standalone layout job for one slice of the note (used for table cells).
pub fn inline_job(cx: &Cx, s: &str, fmt: &TextFormat, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let mut w = W { job: &mut job, lead: 0.0 };
    inline(cx, &mut w, s, fmt);
    if job.text.is_empty() {
        job.append(" ", 0.0, fmt.clone());
    }
    job
}

/// Styles inline Markdown and hides all of its markers.
pub fn inline(cx: &Cx, w: &mut W, s: &str, fmt: &TextFormat) {
    let size = fmt.font_id.size;
    let base_off = cx.off(s);
    let b = s.as_bytes();
    let len = s.len();
    let mut i = 0;
    let mut plain_start = 0;

    macro_rules! flush {
        () => {
            if i > plain_start {
                w.add(&s[plain_start..i], fmt.clone());
            }
        };
    }
    macro_rules! done {
        ($to:expr) => {{
            i = $to;
            plain_start = i;
            continue;
        }};
    }

    while i < len {
        let c = b[i];
        let prev_alnum = i > 0 && (b[i - 1] as char).is_alphanumeric();

        // Backslash escape: hide the backslash.
        if c == b'\\' && i + 1 < len && b[i + 1].is_ascii_punctuation() {
            flush!();
            w.add("\\", hidden());
            w.add(&s[i + 1..i + 2], fmt.clone());
            done!(i + 2);
        }

        // Inline math `$...$` (positions come from the parser).
        if c == b'$' {
            if let Some(&end) = cx.doc.inline_math.get(&(base_off + i)) {
                let end_rel = end - base_off;
                if end_rel <= len {
                    let src = &s[i..end_rel];
                    let inner = src.trim_matches('$');
                    flush!();
                    match visual::math(cx.ui.ctx(), inner, src.starts_with("$$")) {
                        Ok(v) => {
                            // Reserve the math's width before the first (hidden) char; the overlay paints it.
                            let row_h = v.size.y.max(size * 1.25) + 4.0;
                            let first_len = src.chars().next().map_or(1, char::len_utf8);
                            w.lead += v.size.x;
                            w.add(&src[..first_len], TextFormat { line_height: Some(row_h), ..hidden() });
                            w.add(&src[first_len..], hidden());
                        }
                        Err(_) => {
                            w.add(src, TextFormat { font_id: FontId::monospace(size * 0.88), color: Color32::from_rgb(240, 180, 140), ..fmt.clone() });
                        }
                    }
                    done!(end_rel);
                }
            }
        }

        // Obsidian embed / wiki-link: ![[x]] / [[target]] / [[target|alias]]
        let embed = s[i..].starts_with("![[");
        if embed || s[i..].starts_with("[[") {
            let open = if embed { 3 } else { 2 };
            if let Some(rel) = s[i + open..].find("]]") {
                let close = i + open + rel;
                let inner = &s[i + open..close];
                if !inner.is_empty() {
                    flush!();
                    w.add(&s[i..i + open], hidden());
                    match inner.find('|') {
                        Some(p) => {
                            w.add(&inner[..=p], hidden());
                            w.add(&inner[p + 1..], link_format(fmt));
                        }
                        None => w.add(inner, link_format(&TextFormat { italics: embed || fmt.italics, ..fmt.clone() })),
                    }
                    w.add("]]", hidden());
                    done!(close + 2);
                }
            }
        }

        // Footnote reference [^1]
        if s[i..].starts_with("[^") {
            if let Some(rel) = s[i + 2..].find(']') {
                let close = i + 2 + rel;
                let label = &s[i + 2..close];
                if !label.is_empty() && !label.contains(' ') && !s[close + 1..].starts_with(':') {
                    flush!();
                    w.add("[^", hidden());
                    w.add(label, TextFormat { font_id: FontId::proportional(size * 0.72), valign: egui::Align::TOP, ..link_format(fmt) });
                    w.add("]", hidden());
                    done!(close + 1);
                }
            }
        }

        // Image ![alt](src) or link [text](url) / [text][ref]
        let img = c == b'!' && s[i..].starts_with("![");
        if c == b'[' || img {
            let lb = if img { i + 1 } else { i };
            if let Some((label_end, end)) = find_link_end(s, lb) {
                let label = &s[lb + 1..label_end];
                flush!();
                w.add(&s[i..lb + 1], hidden());
                let lf = if img { TextFormat { italics: true, ..link_format(fmt) } } else { link_format(fmt) };
                if img || label.is_empty() {
                    w.add(label, lf);
                } else {
                    inline(cx, w, label, &lf);
                }
                w.add(&s[label_end..end], hidden());
                done!(end);
            }
        }

        // Autolink <https://...> and bare URLs
        if c == b'<' && (s[i + 1..].starts_with("http://") || s[i + 1..].starts_with("https://") || s[i + 1..].starts_with("mailto:")) {
            if let Some(rel) = s[i + 1..].find('>') {
                let close = i + 1 + rel;
                if !s[i + 1..close].contains(' ') {
                    flush!();
                    w.add("<", hidden());
                    w.add(&s[i + 1..close], link_format(fmt));
                    w.add(">", hidden());
                    done!(close + 1);
                }
            }
        }
        if (c == b'h' || c == b'w') && !prev_alnum && (s[i..].starts_with("https://") || s[i..].starts_with("http://") || s[i..].starts_with("www.")) {
            let mut end = s[i..].find(|ch: char| ch.is_whitespace() || ch == '<').map_or(len, |e| i + e);
            while end > i && matches!(b[end - 1], b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b'"' | b'\'') {
                end -= 1;
            }
            if end > i + 8 {
                flush!();
                w.add(&s[i..end], link_format(fmt));
                done!(end);
            }
        }

        // Inline HTML: comments and tags are hidden; known tags style their content.
        if c == b'<' && i + 1 < len && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'/' || b[i + 1] == b'!') {
            if s[i..].starts_with("<!--") {
                if let Some(rel) = s[i..].find("-->") {
                    flush!();
                    w.add(&s[i..i + rel + 3], hidden());
                    done!(i + rel + 3);
                }
            } else if let Some(rel) = s[i..].find('>') {
                let tag_end = i + rel + 1;
                let tag = &s[i..tag_end];
                let name: String = tag[1..].trim_start_matches('/').chars().take_while(|ch| ch.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
                if !name.is_empty() && !tag.contains('\n') {
                    flush!();
                    let closing = format!("</{name}>");
                    if !tag.starts_with("</") && PAIRED_TAGS.contains(&name.as_str()) {
                        if let Some(crel) = s[tag_end..].to_ascii_lowercase().find(&closing) {
                            let cstart = tag_end + crel;
                            w.add(tag, hidden());
                            inline(cx, w, &s[tag_end..cstart], &tag_style(&name, fmt));
                            w.add(&s[cstart..cstart + closing.len()], hidden());
                            done!(cstart + closing.len());
                        }
                    }
                    if name == "summary" && !tag.starts_with("</") {
                        let arrow = cx.glyph_or("▸", ">");
                        w.add(arrow, base(size, colors::ACCENT));
                        let first_len = tag.chars().next().map_or(1, char::len_utf8);
                        w.add(&tag[first_len..], hidden());
                        done!(tag_end);
                    }
                    if name == "img" {
                        let alt = super::doc::html_attr(tag, "alt").unwrap_or_default();
                        if !alt.is_empty() && alt.chars().count() < tag.chars().count() {
                            // Show the alt text in place of the tag's first chars.
                            let n = alt.chars().count();
                            let split = tag.char_indices().nth(n).map_or(tag.len(), |(bi, _)| bi);
                            w.add(&alt, TextFormat { italics: true, ..link_format(fmt) });
                            w.add(&tag[split..], hidden());
                            done!(tag_end);
                        }
                    }
                    w.add(tag, hidden());
                    done!(tag_end);
                }
            }
        }

        // `code` (also ``code with ` inside``)
        if c == b'`' {
            let ticks = b[i..].iter().take_while(|&&t| t == b'`').count();
            let delim = &s[i..i + ticks];
            if let Some(rel) = s[i + ticks..].find(delim) {
                let close = i + ticks + rel;
                flush!();
                let code = TextFormat {
                    font_id: FontId::monospace(size * 0.88),
                    color: Color32::from_rgb(240, 180, 140),
                    background: CODE_BG,
            expand_bg: 1.5,
                    italics: false,
                    ..fmt.clone()
                };
                w.add(delim, hidden());
                w.add(&s[i + ticks..close], code);
                w.add(delim, hidden());
                done!(close + ticks);
            } else {
                // Unmatched run of backticks: skip it whole.
                i += ticks;
                continue;
            }
        }

        // Emoji shortcode :smile:
        if c == b':' && i + 2 < len && b[i + 1] != b' ' {
            if let Some(rel) = s[i + 1..].find(':') {
                let close = i + 1 + rel;
                let name = &s[i + 1..close];
                if !name.is_empty() && name.len() < 40 && name.bytes().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'+' | b'-')) {
                    if let Some(e) = emojis::get_by_shortcode(name) {
                        let shown = e.as_str();
                        let src = &s[i..=close];
                        let n = shown.chars().count();
                        if n <= src.chars().count() {
                            flush!();
                            let split = src.char_indices().nth(n).map_or(src.len(), |(bi, _)| bi);
                            w.add(shown, base(size, fmt.color));
                            w.add(&src[split..], hidden());
                            done!(close + 1);
                        }
                    }
                }
            }
        }

        // **bold** / __bold__ / ~~strike~~ / ==highlight==
        let mut matched = None;
        for delim in ["**", "__", "~~", "=="] {
            if !s[i..].starts_with(delim) || (delim == "__" && prev_alnum) {
                continue;
            }
            if let Some(rel) = s[i + 2..].find(delim) {
                let close = i + 2 + rel;
                let content = &s[i + 2..close];
                if content.is_empty() || content.starts_with(' ') || content.ends_with(' ') {
                    continue;
                }
                matched = Some((delim, close));
                break;
            }
        }
        if let Some((delim, close)) = matched {
            flush!();
            let styled = match delim {
                "**" | "__" => tag_style("b", fmt),
                "~~" => tag_style("s", fmt),
                _ => tag_style("mark", fmt),
            };
            w.add(delim, hidden());
            inline(cx, w, &s[i + 2..close], &styled);
            w.add(delim, hidden());
            done!(close + 2);
        }

        // *italic* / _italic_ / ~sub~-style single tilde strike
        if (c == b'*' || (c == b'_' && !prev_alnum)) && i + 1 < len && b[i + 1] != b' ' && b[i + 1] != c {
            let d = c as char;
            if let Some(rel) = s[i + 1..].find(d) {
                let close = i + 1 + rel;
                let next_alnum = close + 1 < len && (b[close + 1] as char).is_alphanumeric();
                if close > i + 1 && !(d == '_' && next_alnum) && b[close - 1] != b' ' {
                    flush!();
                    w.add(&s[i..=i], hidden());
                    inline(cx, w, &s[i + 1..close], &tag_style("i", fmt));
                    w.add(&s[close..=close], hidden());
                    done!(close + 1);
                }
            }
        }

        // Plain character: advance by one full UTF-8 char.
        i += s[i..].chars().next().map_or(1, char::len_utf8);
    }

    if len > plain_start {
        w.add(&s[plain_start..len], fmt.clone());
    }
}

/// For `[label](dest)` / `[label][ref]` starting at `lb` (the `[`), returns
/// (index of the `]`, end index just past the link). Handles nested brackets in the label.
pub fn find_link_end(s: &str, lb: usize) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let mut depth = 0;
    let mut j = lb;
    let label_end = loop {
        if j >= b.len() {
            return None;
        }
        match b[j] {
            b'\\' => j += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break j;
                }
            }
            _ => {}
        }
        j += 1;
    };
    match b.get(label_end + 1) {
        Some(b'(') => {
            let mut depth = 0;
            let mut k = label_end + 1;
            while k < b.len() {
                match b[k] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some((label_end, k + 1));
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            None
        }
        Some(b'[') => {
            let close = s[label_end + 2..].find(']')? + label_end + 2;
            Some((label_end, close + 1))
        }
        _ => None,
    }
}
