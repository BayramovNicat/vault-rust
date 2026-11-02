//! Turns rich blocks into things egui can draw: SVGs for diagrams and math,
//! image URIs for pictures, and colors for highlighted code.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use egui::{Color32, Context, Vec2};

/// How long a looked-up image path is trusted before checking the disk again.
const PATH_TTL: Duration = Duration::from_secs(2);

/// Hashes several parts without joining them into one string first.
fn hash_parts(parts: &[&str]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// Where relative image paths are looked up.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub note_dir: PathBuf,
    pub vault_root: PathBuf,
}

/// An SVG that has been handed to egui's image loaders.
#[derive(Clone, Debug)]
pub struct SvgVisual {
    pub uri: String,
    /// Natural size in points.
    pub size: Vec2,
}

/// Vault root, file name (lowercase) -> path, and when it was built.
type FileIndex = (PathBuf, HashMap<String, PathBuf>, Instant);

thread_local! {
    static SVGS: RefCell<HashMap<u64, Result<SvgVisual, String>>> = RefCell::new(HashMap::new());
    /// File name (lowercase) -> path, for the whole vault, and when it was built.
    static FILE_INDEX: RefCell<Option<FileIndex>> = const { RefCell::new(None) };
    /// (note folder, image src) -> resolved URI, and when it was resolved.
    static IMAGE_URIS: RefCell<HashMap<(PathBuf, String), (String, Instant)>> = RefCell::new(HashMap::new());
}

/// Rewrites the root `<svg>` tag so it has plain pixel `width`/`height` taken from its viewBox,
/// scaled by `scale`. Returns the new SVG and its size.
fn normalize_svg(svg: &str, scale: f32) -> Option<(String, Vec2)> {
    let start = svg.find("<svg")?;
    let end = start + svg[start..].find('>')?;
    let tag = &svg[start..=end];
    let vb = super::doc::html_attr(tag, "viewbox")?;
    let nums: Vec<f32> = vb.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|n| n.parse().ok()).collect();
    if nums.len() != 4 || nums[2] <= 0.0 || nums[3] <= 0.0 {
        return None;
    }
    let size = Vec2::new(nums[2] * scale, nums[3] * scale);

    // Drop width/height/style from the root tag, then add our own.
    let mut new_tag = String::from("<svg");
    let mut rest = &tag[4..tag.len() - 1];
    let self_closing = rest.trim_end().ends_with('/');
    while let Some(eq) = rest.find('=') {
        let name = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let (value, remaining) = match after.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let close = after[1..].find(q).map_or(after.len(), |i| i + 2);
                (&after[..close], &after[close..])
            }
            _ => {
                let close = after.find(char::is_whitespace).unwrap_or(after.len());
                (&after[..close], &after[close..])
            }
        };
        let lname = name.to_ascii_lowercase();
        if lname != "width" && lname != "height" && lname != "style" && !name.is_empty() {
            new_tag.push(' ');
            new_tag.push_str(&name);
            new_tag.push('=');
            new_tag.push_str(value);
        }
        rest = remaining;
    }
    new_tag.push_str(&format!(" width=\"{}\" height=\"{}\"", size.x, size.y));
    new_tag.push_str(if self_closing { "/>" } else { ">" });

    Some((format!("{}{}{}", &svg[..start], new_tag, &svg[end + 1..]), size))
}

fn svg_visual(ctx: &Context, kind: &str, key: &str, scale: f32, render: impl FnOnce() -> Result<String, String>) -> Result<SvgVisual, String> {
    let h = hash_parts(&[kind, key]);
    if let Some(v) = SVGS.with(|s| s.borrow().get(&h).cloned()) {
        return v;
    }
    let result = render().and_then(|svg| {
        let (svg, size) = normalize_svg(&svg, scale).ok_or_else(|| "could not read SVG size".to_string())?;
        let uri = format!("bytes://{kind}-{h:x}.svg");
        ctx.include_bytes(uri.clone(), svg.into_bytes());
        Ok(SvgVisual { uri, size })
    });
    SVGS.with(|s| s.borrow_mut().insert(h, result.clone()));
    result
}

pub fn mermaid(ctx: &Context, src: &str) -> Result<SvgVisual, String> {
    svg_visual(ctx, "mermaid", src, 1.0, || {
        let mut theme = mermaid_rs_renderer::Theme::dark();
        // Match the app background and use a bright palette that reads well on dark.
        theme.background = "transparent".to_string();
        theme.pie_stroke_color = "#1a1a1f".to_string();
        theme.pie_colors = [
            "#5582f0", "#41c36e", "#eba028", "#eb5050", "#ab7df8", "#2ec4c4",
            "#f07ab4", "#a0c850", "#e6783c", "#7896ff", "#c8a064", "#64b4dc",
        ]
        .map(String::from);
        let opts = mermaid_rs_renderer::RenderOptions { theme, layout: Default::default() };
        std::panic::catch_unwind(|| mermaid_rs_renderer::render_with_options(src, opts))
            .map_err(|_| "diagram renderer crashed".to_string())?
            .map_err(|e| e.to_string())
    })
}

fn math_font() -> Option<&'static latex_rust::MathFont> {
    static FONT: OnceLock<Option<latex_rust::MathFont>> = OnceLock::new();
    FONT.get_or_init(|| latex_rust::MathFont::stix_two_math().ok()).as_ref()
}

/// Math is rendered in pt; 1pt = 4/3 px.
pub fn math(ctx: &Context, src: &str, display: bool) -> Result<SvgVisual, String> {
    let kind = if display { "math-display" } else { "math" };
    svg_visual(ctx, kind, src, 4.0 / 3.0, || {
        let font = math_font().ok_or("math font missing")?;
        let mut opts = latex_rust::SvgOptions::new();
        opts.font_size_pt = latex_rust::Dim::from_i64(if display { 13 } else { 11 });
        opts.color = latex_rust::Color::rgb(235, 235, 240);
        opts.display = display;
        std::panic::catch_unwind(|| latex_rust::latex_to_svg(src, font, &opts))
            .map_err(|_| "math renderer crashed".to_string())?
            .map_err(|e| e.to_string())
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Finds a file anywhere in the vault by name (for Obsidian-style `![[image.png]]`).
fn find_in_vault(root: &Path, name: &str) -> Option<PathBuf> {
    let key = name.to_lowercase();
    let (cached, fresh) = FILE_INDEX.with(|f| match f.borrow().as_ref() {
        Some((r, idx, built)) if r == root => (idx.get(&key).cloned(), built.elapsed() < PATH_TTL),
        _ => (None, false),
    });
    if cached.as_ref().is_some_and(|p| p.exists()) {
        return cached;
    }
    // Missing: the file may be new, but walk the vault at most once per PATH_TTL.
    if fresh {
        return None;
    }
    let mut idx = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            seen += 1;
            if seen > 20_000 {
                break;
            }
            let p = e.path();
            let fname = e.file_name().to_string_lossy().to_string();
            if fname.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                idx.entry(fname.to_lowercase()).or_insert(p);
            }
        }
    }
    let found = idx.get(&key).cloned();
    FILE_INDEX.with(|f| *f.borrow_mut() = Some((root.to_path_buf(), idx, Instant::now())));
    found
}

/// Turns an image reference from a note into a URI egui's loaders understand.
/// Local lookups are cached briefly: this runs for every image on every frame.
pub fn image_uri(src: &str, env: &Env) -> String {
    let src = src.trim();
    if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("bytes://") || src.starts_with("file://") {
        return src.to_string();
    }
    let key = (env.note_dir.clone(), src.to_string());
    if let Some(uri) = IMAGE_URIS.with(|m| m.borrow().get(&key).filter(|(_, at)| at.elapsed() < PATH_TTL).map(|(u, _)| u.clone())) {
        return uri;
    }
    let uri = resolve_local_image(src, env);
    IMAGE_URIS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 1000 {
            m.clear();
        }
        m.insert(key, (uri.clone(), Instant::now()));
    });
    uri
}

fn resolve_local_image(src: &str, env: &Env) -> String {
    let clean = percent_decode(src.split(['?', '#']).next().unwrap_or(src));
    let p = Path::new(&clean);
    let candidates = if p.is_absolute() {
        vec![p.to_path_buf()]
    } else {
        vec![env.note_dir.join(p), env.vault_root.join(p)]
    };
    let found = candidates
        .iter()
        .find(|c| c.exists())
        .cloned()
        .or_else(|| p.file_name().and_then(|n| find_in_vault(&env.vault_root, &n.to_string_lossy())))
        .unwrap_or_else(|| candidates[0].clone());
    format!("file://{}", found.display())
}

pub enum ImageState {
    Ready(Vec2),
    Loading(Option<Vec2>),
    Failed(String),
}

pub fn image_state(ctx: &Context, uri: &str) -> ImageState {
    match ctx.try_load_texture(uri, egui::TextureOptions::LINEAR, egui::SizeHint::default()) {
        Ok(egui::load::TexturePoll::Ready { texture }) => ImageState::Ready(texture.size),
        Ok(egui::load::TexturePoll::Pending { size }) => ImageState::Loading(size),
        Err(e) => ImageState::Failed(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Code highlighting
// ---------------------------------------------------------------------------

/// Per line: (byte range within the line, color, italic).
pub type Highlight = Arc<Vec<Vec<(Range<usize>, Color32, bool)>>>;

thread_local! {
    static HIGHLIGHTS: RefCell<HashMap<u64, Highlight>> = RefCell::new(HashMap::new());
}

fn syntect_assets() -> &'static (syntect::parsing::SyntaxSet, syntect::highlighting::Theme) {
    static ASSETS: OnceLock<(syntect::parsing::SyntaxSet, syntect::highlighting::Theme)> = OnceLock::new();
    ASSETS.get_or_init(|| {
        let ss = syntect::parsing::SyntaxSet::load_defaults_newlines();
        let mut themes = syntect::highlighting::ThemeSet::load_defaults();
        let theme = themes.themes.remove("base16-ocean.dark").unwrap_or_default();
        (ss, theme)
    })
}

fn lang_alias(lang: &str) -> &str {
    match lang {
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "json" | "jsonc" => "js",
        "py" | "python" => "py",
        "rs" | "rust" => "rs",
        "sh" | "bash" | "zsh" | "shell" | "console" => "sh",
        "yml" | "yaml" => "yaml",
        "md" | "markdown" => "md",
        "c++" | "cpp" | "cc" | "hpp" => "cpp",
        "cs" | "csharp" => "cs",
        "rb" | "ruby" => "rb",
        "golang" | "go" => "go",
        "toml" => "yaml",
        other => other,
    }
}

/// Syntax colors for a code block's lines. `None` for unknown languages.
pub fn highlight(lang: &str, code: &str) -> Option<Highlight> {
    use syntect::easy::HighlightLines;
    use syntect::util::LinesWithEndings;

    if lang.is_empty() {
        return None;
    }
    let h = hash_parts(&[lang, code]);
    if let Some(v) = HIGHLIGHTS.with(|c| c.borrow().get(&h).cloned()) {
        return Some(v);
    }
    let (ss, theme) = syntect_assets();
    let syntax = ss.find_syntax_by_token(lang_alias(lang)).or_else(|| ss.find_syntax_by_token(lang))?;
    let mut hl = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(code) {
        let mut spans = Vec::new();
        let mut at = 0;
        if let Ok(regions) = hl.highlight_line(line, ss) {
            for (style, piece) in regions {
                let piece_trimmed = piece.trim_end_matches(['\n', '\r']);
                let fg = style.foreground;
                spans.push((
                    at..at + piece_trimmed.len(),
                    Color32::from_rgb(fg.r, fg.g, fg.b),
                    style.font_style.contains(syntect::highlighting::FontStyle::ITALIC),
                ));
                at += piece.len();
            }
        }
        lines.push(spans);
    }
    let result: Highlight = Arc::new(lines);
    HIGHLIGHTS.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 500 {
            c.clear();
        }
        c.insert(h, result.clone());
    });
    Some(result)
}
