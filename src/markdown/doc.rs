//! First pass over a note: finds the blocks that need more than styled text
//! (images, diagrams, math, tables, code, quotes, ...) and their byte ranges.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use super::visual::{self, Highlight};

#[derive(Clone, Debug)]
pub struct ImageRef {
    pub src: String,
    pub alt: String,
    /// Set when the image is wrapped in a link (e.g. README badges).
    pub link: Option<String>,
    /// `width="..."` from an HTML `<img>`.
    pub width: Option<f32>,
}

#[derive(Clone, Debug)]
pub enum RichKind {
    /// A paragraph or HTML block made only of images.
    Images(Vec<ImageRef>),
    Mermaid(String),
    /// Display math (`$$ .. $$` or a ```math fence).
    Math(String),
    /// Byte ranges of each cell's content; row 0 is the header.
    Table { aligns: Vec<Alignment>, rows: TableCells },
    /// Shown as nothing at all: HTML comments, link reference definitions.
    Hidden,
}

/// A block drawn as a widget in a reserved gap. `range` covers whole lines (no trailing `\n`).
#[derive(Clone, Debug)]
pub struct RichBlock {
    pub range: Range<usize>,
    pub kind: RichKind,
}

#[derive(Clone, Debug)]
pub struct CodeBlock {
    /// Whole lines, fences included.
    pub range: Range<usize>,
    pub lang: String,
    pub fenced: bool,
    /// The code itself: between the fences, or all of `range` when indented.
    pub body: Range<usize>,
    /// Start of each line of `body`.
    body_lines: Vec<usize>,
    highlight: OnceLock<Option<Highlight>>,
}

pub fn is_fence_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

impl CodeBlock {
    fn new(text: &str, range: Range<usize>, lang: String, fenced: bool) -> Self {
        let block = &text[range.clone()];
        let body = if fenced {
            let first_nl = block.find('\n').map_or(block.len(), |i| i + 1);
            let last_nl = block.rfind('\n').unwrap_or(0);
            let closed = last_nl >= first_nl && block.get(last_nl + 1..).is_some_and(is_fence_line);
            let end = if closed { last_nl } else { block.len() };
            range.start + first_nl.min(end)..range.start + end
        } else {
            range.clone()
        };
        let body_lines = std::iter::once(body.start)
            .chain(text[body.clone()].match_indices('\n').map(|(i, _)| body.start + i + 1))
            .collect();
        Self { range, lang, fenced, body, body_lines, highlight: OnceLock::new() }
    }

    /// Index of the body line starting at `line_start`, if it is one.
    pub fn body_line(&self, line_start: usize) -> Option<usize> {
        if line_start < self.body.start || line_start > self.body.end {
            return None;
        }
        Some(self.body_lines.partition_point(|&s| s <= line_start).saturating_sub(1))
    }

    /// Syntax colors per body line (computed once per parsed note).
    pub fn highlight(&self, text: &str) -> Option<&Highlight> {
        self.highlight.get_or_init(|| visual::highlight(&self.lang, &text[self.body.clone()])).as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone, Debug)]
pub struct Quote {
    pub range: Range<usize>,
    pub depth: usize,
    pub alert: Option<Alert>,
}

#[derive(Default)]
pub struct Doc {
    pub hash: u64,
    pub rich: Vec<RichBlock>,
    pub code: Vec<CodeBlock>,
    pub quotes: Vec<Quote>,
    /// `[ ]` / `[x]` marker ranges.
    pub tasks: Vec<(Range<usize>, bool)>,
    /// Inline math: start of the `$` -> end (exclusive) of the closing `$`.
    pub inline_math: HashMap<usize, usize>,
    pub frontmatter: Option<Range<usize>>,
    pub footnote_defs: Vec<Range<usize>>,
    /// Line start of a setext heading's title line -> level.
    pub setext_titles: HashMap<usize, u8>,
    /// Line starts of setext underline (`===` / `---`) lines.
    pub setext_underlines: HashSet<usize>,
    /// Line starts of horizontal rules.
    pub rules: HashSet<usize>,
    /// Each heading: (line start, level, anchor slug for `[x](#anchor)` links).
    pub headings: Vec<(usize, u8, String)>,
    char_starts: OnceLock<Vec<usize>>,
}

impl Doc {
    pub fn rich_at(&self, line_start: usize) -> Option<&RichBlock> {
        self.rich.iter().find(|b| b.range.start <= line_start && line_start <= b.range.end)
    }
    pub fn code_at(&self, line_start: usize) -> Option<&CodeBlock> {
        self.code.iter().find(|b| b.range.start <= line_start && line_start <= b.range.end)
    }
    pub fn quote_depth_at(&self, line_start: usize) -> Option<&Quote> {
        self.quotes
            .iter()
            .filter(|q| q.range.start <= line_start && line_start <= q.range.end)
            .max_by_key(|q| q.depth)
    }
    pub fn alert_at(&self, line_start: usize) -> Option<Alert> {
        self.quotes
            .iter()
            .filter(|q| q.range.start <= line_start && line_start <= q.range.end)
            .find_map(|q| q.alert)
    }
    pub fn in_footnote_def(&self, line_start: usize) -> bool {
        self.footnote_defs.iter().any(|r| r.start <= line_start && line_start <= r.end)
    }
    /// Byte offset of every char of `text` (the text this doc was parsed from).
    pub fn char_starts(&self, text: &str) -> &[usize] {
        self.char_starts.get_or_init(|| text.char_indices().map(|(b, _)| b).collect())
    }
}

pub fn hash_text(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

thread_local! {
    static CACHE: RefCell<Option<Arc<Doc>>> = const { RefCell::new(None) };
}

/// Parsed info for `text`, cached for the last text seen.
pub fn doc_for(text: &str) -> Arc<Doc> {
    let h = hash_text(text);
    CACHE.with(|c| {
        if let Some(d) = c.borrow().as_ref() {
            if d.hash == h {
                return d.clone();
            }
        }
        let d = Arc::new(parse(text, h));
        *c.borrow_mut() = Some(d.clone());
        d
    })
}

/// Expands a byte range to whole lines (excluding the final newline).
pub fn line_span(text: &str, r: &Range<usize>) -> Range<usize> {
    let start = text[..r.start.min(text.len())].rfind('\n').map_or(0, |i| i + 1);
    let mut last = r.end.min(text.len());
    if last > start && text.as_bytes()[last - 1] == b'\n' {
        last -= 1;
    }
    let end = text[last..].find('\n').map_or(text.len(), |i| last + i);
    start..end.max(start)
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c == ' ' {
            out.push('-');
        }
    }
    out
}

pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_WIKILINKS
}

/// Byte ranges of each table cell, row by row.
type TableCells = Vec<Vec<Range<usize>>>;

struct Para {
    start: usize,
    only_images: bool,
    images: Vec<ImageRef>,
    in_image: Option<ImageRef>,
    link: Vec<String>,
}

fn parse(text: &str, hash: u64) -> Doc {
    let mut doc = Doc { hash, ..Default::default() };
    let parser = Parser::new_ext(text, options());

    for (_, def) in parser.reference_definitions().iter() {
        doc.rich.push(RichBlock { range: line_span(text, &def.span), kind: RichKind::Hidden });
    }

    let mut para: Option<Para> = None;
    let mut code: Option<(usize, String, bool, String)> = None;
    let mut quote_stack: Vec<(usize, Option<Alert>)> = Vec::new();
    let mut table: Option<(usize, Vec<Alignment>, TableCells)> = None;
    let mut html: Option<(usize, String)> = None;
    let mut heading: Option<(usize, String)> = None;

    for (event, range) in parser.into_offset_iter() {
        // Anything other than images inside a paragraph means it's normal text.
        if let Some(p) = &mut para {
            match &event {
                Event::Start(Tag::Image { dest_url, .. }) => {
                    p.in_image = Some(ImageRef {
                        src: dest_url.to_string(),
                        alt: String::new(),
                        link: p.link.last().cloned(),
                        width: None,
                    });
                }
                Event::End(TagEnd::Image) => {
                    if let Some(img) = p.in_image.take() {
                        p.images.push(img);
                    }
                }
                Event::Text(t) if p.in_image.is_some() => {
                    if let Some(img) = &mut p.in_image {
                        img.alt.push_str(t);
                    }
                }
                Event::Start(Tag::Link { dest_url, .. }) => p.link.push(dest_url.to_string()),
                Event::End(TagEnd::Link) => {
                    p.link.pop();
                }
                Event::Text(t) if t.trim().is_empty() => {}
                Event::SoftBreak | Event::HardBreak | Event::End(TagEnd::Paragraph) => {}
                _ if p.in_image.is_some() => {}
                _ => p.only_images = false,
            }
        }

        if let Some((_, buf)) = &mut heading {
            if let Event::Text(t) | Event::Code(t) = &event {
                buf.push_str(t);
            }
        }

        match event {
            Event::Start(Tag::Paragraph) => {
                para = Some(Para { start: range.start, only_images: true, images: vec![], in_image: None, link: vec![] });
            }
            Event::End(TagEnd::Paragraph) => {
                if let Some(p) = para.take() {
                    if p.only_images && !p.images.is_empty() {
                        doc.rich.push(RichBlock { range: line_span(text, &(p.start..range.end)), kind: RichKind::Images(p.images) });
                    }
                }
            }
            Event::Start(Tag::Heading { .. }) => heading = Some((range.start, String::new())),
            Event::End(TagEnd::Heading(level)) => {
                if let Some((start, title)) = heading.take() {
                    let span = line_span(text, &(start..range.end));
                    doc.headings.push((span.start, level as u8, slugify(&title)));
                    if !text[span.clone()].trim_start().starts_with('#') {
                        // Setext heading: title line(s) + underline line.
                        let last_line = text[span.clone()].rfind('\n').map_or(span.start, |i| span.start + i + 1);
                        doc.setext_titles.insert(span.start, level as u8);
                        doc.setext_underlines.insert(last_line);
                    }
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let (lang, fenced) = match kind {
                    CodeBlockKind::Fenced(l) => (l.split_whitespace().next().unwrap_or("").to_lowercase(), true),
                    CodeBlockKind::Indented => (String::new(), false),
                };
                code = Some((range.start, lang, fenced, String::new()));
            }
            Event::Text(t) if code.is_some() => {
                if let Some((_, _, _, body)) = &mut code {
                    body.push_str(&t);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((start, lang, fenced, body)) = code.take() {
                    let span = line_span(text, &(start..range.end));
                    match lang.as_str() {
                        "mermaid" => doc.rich.push(RichBlock { range: span, kind: RichKind::Mermaid(body) }),
                        "math" | "latex" | "tex" | "katex" => {
                            doc.rich.push(RichBlock { range: span, kind: RichKind::Math(body.trim().to_string()) })
                        }
                        _ => doc.code.push(CodeBlock::new(text, span, lang, fenced)),
                    }
                }
            }
            Event::DisplayMath(m) => {
                let span = line_span(text, &range);
                let alone = text[span.start..range.start].trim().is_empty() && text[range.end..span.end].trim().is_empty();
                if alone {
                    doc.rich.push(RichBlock { range: span, kind: RichKind::Math(m.trim().to_string()) });
                } else {
                    doc.inline_math.insert(range.start, range.end);
                }
            }
            Event::InlineMath(_) => {
                doc.inline_math.insert(range.start, range.end);
            }
            Event::Start(Tag::BlockQuote(kind)) => {
                let alert = kind.map(|k| match k {
                    BlockQuoteKind::Note => Alert::Note,
                    BlockQuoteKind::Tip => Alert::Tip,
                    BlockQuoteKind::Important => Alert::Important,
                    BlockQuoteKind::Warning => Alert::Warning,
                    BlockQuoteKind::Caution => Alert::Caution,
                });
                quote_stack.push((range.start, alert));
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                let depth = quote_stack.len();
                if let Some((start, alert)) = quote_stack.pop() {
                    doc.quotes.push(Quote { range: line_span(text, &(start..range.end)), depth, alert });
                }
            }
            Event::TaskListMarker(checked) => doc.tasks.push((range, checked)),
            Event::Rule => {
                doc.rules.insert(line_span(text, &range).start);
            }
            Event::Start(Tag::MetadataBlock(_)) => {
                doc.frontmatter = Some(line_span(text, &range));
            }
            Event::Start(Tag::FootnoteDefinition(_)) => {
                doc.footnote_defs.push(line_span(text, &range));
            }
            Event::Start(Tag::Table(aligns)) => table = Some((range.start, aligns, Vec::new())),
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                if let Some((_, _, rows)) = &mut table {
                    rows.push(Vec::new());
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some((_, _, rows)) = &mut table {
                    if let Some(row) = rows.last_mut() {
                        row.push(trim_range(text, range));
                    }
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some((start, aligns, rows)) = table.take() {
                    doc.rich.push(RichBlock { range: line_span(text, &(start..range.end)), kind: RichKind::Table { aligns, rows } });
                }
            }
            Event::Start(Tag::HtmlBlock) => html = Some((range.start, String::new())),
            Event::Html(h) => {
                if let Some((_, buf)) = &mut html {
                    buf.push_str(&h);
                }
            }
            Event::End(TagEnd::HtmlBlock) => {
                if let Some((start, src)) = html.take() {
                    let span = line_span(text, &(start..range.end));
                    let trimmed = src.trim();
                    if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
                        doc.rich.push(RichBlock { range: span, kind: RichKind::Hidden });
                    } else if let Some(images) = html_only_images(trimmed) {
                        doc.rich.push(RichBlock { range: span, kind: RichKind::Images(images) });
                    }
                }
            }
            _ => {}
        }
    }

    doc.rich.sort_by_key(|b| b.range.start);
    doc
}

fn trim_range(text: &str, r: Range<usize>) -> Range<usize> {
    let s = &text[r.clone()];
    let lead = s.len() - s.trim_start().len();
    let trail = s.len() - s.trim_end().len();
    (r.start + lead)..(r.end - trail).max(r.start + lead)
}

/// Value of an HTML attribute like `src="..."` inside one tag.
pub fn html_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = tag[at + name.len()..].trim_start();
        if before_ok {
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim_start();
                let (q, body) = match rest.chars().next() {
                    Some(q @ ('"' | '\'')) => (Some(q), &rest[1..]),
                    _ => (None, rest),
                };
                let end = match q {
                    Some(q) => body.find(q),
                    None => body.find(|c: char| c.is_whitespace() || c == '>' || c == '/'),
                }
                .unwrap_or(body.len());
                return Some(body[..end].to_string());
            }
        }
        from = at + name.len();
    }
    None
}

/// If an HTML block contains `<img>` tags and no other visible text, returns those images.
fn html_only_images(html: &str) -> Option<Vec<ImageRef>> {
    let mut images = Vec::new();
    let mut visible = String::new();
    let mut link: Option<String> = None;
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        visible.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else { break };
        let tag = &rest[open..open + close + 1];
        let lower = tag.to_ascii_lowercase();
        if lower.starts_with("<img") {
            if let Some(src) = html_attr(tag, "src") {
                images.push(ImageRef {
                    src,
                    alt: html_attr(tag, "alt").unwrap_or_default(),
                    link: link.clone(),
                    width: html_attr(tag, "width").and_then(|w| w.trim_end_matches("px").parse().ok()),
                });
            }
        } else if lower.starts_with("<a ") || lower.starts_with("<a>") {
            link = html_attr(tag, "href");
        } else if lower.starts_with("</a") {
            link = None;
        }
        rest = &rest[open + close + 1..];
    }
    visible.push_str(rest);
    let visible = visible.replace("&nbsp;", "");
    (!images.is_empty() && visible.trim().is_empty()).then_some(images)
}
