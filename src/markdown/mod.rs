//! Live-preview Markdown editing (Obsidian / Typora style).
//!
//! - `doc`: first pass that finds rich blocks (images, diagrams, math, tables, code, quotes).
//! - `layout`: the text-box layouter that hides syntax and reserves space for rich blocks.
//! - `overlay`: paints rich content and decorations into that space.
//! - `visual`: renders Mermaid / LaTeX to SVG, resolves image paths, highlights code.

mod doc;
mod layout;
mod overlay;
mod svg_loader;
mod visual;

use std::ops::Range;

pub use layout::live_layouter;
pub use overlay::paint as paint_overlay;
pub use svg_loader::SvgByContentLoader;
pub use visual::Env;

/// Something the user clicked in the live preview that the app should handle.
pub enum PreviewAction {
    /// Open (or create) a note: from a `[[wiki-link]]` or a relative `[link](note.md)`.
    OpenNote(String),
    /// Open an external URL in the system browser.
    OpenUrl(String),
    /// Flip the task checkbox whose `[ ]` / `[x]` marker sits at this byte range.
    ToggleTask(Range<usize>),
    /// Move the cursor to this byte offset (a `[x](#heading)` link).
    Jump(usize),
    /// Replace this byte range of the note with new text (table cell edits).
    Replace(Range<usize>, String),
}

/// Works out what a click at byte offset `pos` should do.
/// Links open with Cmd+click (a plain click just places the cursor); checkboxes toggle on a plain click.
pub fn click_action(text: &str, pos: usize, command: bool) -> Option<PreviewAction> {
    let d = doc::doc_for(text);
    if !command {
        return d
            .tasks
            .iter()
            .find(|(r, _)| r.start <= pos && pos <= r.end)
            .map(|(r, _)| PreviewAction::ToggleTask(r.clone()));
    }

    let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    let line = &text[line_start..line_end];
    let col = pos - line_start;

    // [[wiki]] / ![[embed]]
    let mut search = 0;
    while let Some(rel) = line[search..].find("[[") {
        let open = search + rel;
        let Some(close_rel) = line[open + 2..].find("]]") else { break };
        let close = open + 2 + close_rel;
        if open <= col && col <= close + 2 {
            let inner = &line[open + 2..close];
            let target = inner.split('|').next().unwrap_or(inner).trim();
            let target = target.split('#').next().unwrap_or(target);
            return (!target.is_empty()).then(|| PreviewAction::OpenNote(target.to_string()));
        }
        search = close + 2;
    }

    // [label](dest)
    let mut search = 0;
    while let Some(rel) = line[search..].find('[') {
        let open = search + rel;
        let Some((label_end, end)) = layout::find_link_end(line, open) else {
            search = open + 1;
            continue;
        };
        if open <= col && col <= end && line.as_bytes().get(label_end + 1) == Some(&b'(') {
            let dest = line[label_end + 2..end - 1].trim();
            let dest = dest.split_whitespace().next().unwrap_or("").trim_matches(['<', '>']);
            return link_target(&d, dest);
        }
        search = end.max(open + 1);
    }

    // <https://...> and bare URLs
    for (i, _) in line.match_indices("http") {
        if i > col {
            break;
        }
        let end = line[i..].find(|c: char| c.is_whitespace() || c == '>' || c == ')').map_or(line.len(), |e| i + e);
        if col <= end && (line[i..].starts_with("http://") || line[i..].starts_with("https://")) {
            return Some(PreviewAction::OpenUrl(line[i..end].trim_end_matches(['.', ',']).to_string()));
        }
    }
    None
}

fn link_target(d: &doc::Doc, dest: &str) -> Option<PreviewAction> {
    if dest.is_empty() {
        return None;
    }
    if let Some(anchor) = dest.strip_prefix('#') {
        let slug = doc::slugify(anchor);
        return d.headings.iter().find(|(_, _, s)| *s == slug).map(|(at, _, _)| PreviewAction::Jump(*at));
    }
    if dest.contains("://") || dest.starts_with("mailto:") {
        return Some(PreviewAction::OpenUrl(dest.to_string()));
    }
    Some(PreviewAction::OpenNote(dest.split('#').next().unwrap_or(dest).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"---
title: Test
tags: [a, b]
---

# Привет **мир**

    indented code

Setext Title
============

Some **bold**, *it*, _it2_, ~~strike~~, ==mark==, `code`, ``a ` b``, \*not italic\*, H~2~O.
Inline math $E = mc^2$ and $$x^2$$ here. Emoji :smile: :+1: :not_an_emoji:.
Links: [[Note|alias]] ![[pic.png]] [web](https://a.b) <https://c.d> https://e.f/g. [ref][r] [^1]
HTML: <kbd>Ctrl</kbd> <mark>hi</mark> <sub>2</sub> <sup>3</sup> <u>u</u> <br> <!-- hidden -->

- item
  - nested
    - deeper
1. one
2. [ ] ordered task
- [ ] todo
- [x] done

> quote **bold**
> > nested quote

> [!WARNING]
> careful

| Left | Center | Right |
|:-----|:------:|------:|
| a    | **b**  | `c`   |

```rust
fn main() { println!("hi"); }
```

```mermaid
graph TD; A-->B;
```

$$
\frac{1}{2}
$$

![alt](img.png)

<p align="center"><img src="https://x.y/z.png" width="100"></p>

<details>
<summary>More</summary>
Body
</details>

Term
: Definition

---

[r]: https://ref.example
[^1]: Footnote text.
"#;

    /// Every source char must stay in the layout, or cursor positions drift.
    #[test]
    fn layout_keeps_every_char() {
        let ctx = egui::Context::default();
        let env = Env::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for reveal in [None, Some((0, SAMPLE.len()))] {
                    for text in [SAMPLE, include_str!("../../docs/showcase.md"), "", "\n", "#", "```", "$$", "| a |\n|---|", "**unclosed *also [[open `tick"] {
                        let g = live_layouter(ui, text, 500.0, reveal, &env);
                        assert_eq!(g.job.text.chars().count(), text.chars().count(), "reveal={reveal:?} text={text:?}\nlayout={:?}", g.job.text);
                    }
                }
            });
        });
    }

    /// Blank lines next to rich blocks keep normal height (no doubled gaps).
    #[test]
    fn blank_lines_keep_normal_height() {
        let ctx = egui::Context::default();
        let env = Env::default();
        let text = "Intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nlet x = 1;\n```\n\nOutro";
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let g = live_layouter(ui, text, 600.0, None, &env);
                let heights: Vec<f32> = g.rows.iter().map(|r| r.rect().height()).collect();
                // Rows: Intro, blank, table gap, (2 collapsed), blank, fence, code, fence, blank, Outro
                let body = heights[0];
                for &i in &[1usize, 5, 9] {
                    assert!((heights[i] - body).abs() < 1.0, "blank row {i} is {} (body {body}); all: {heights:?}", heights[i]);
                }
                assert!(heights[3] < 1.0 && heights[4] < 1.0, "table source rows should collapse: {heights:?}");
            });
        });
    }

    /// Characters the built-in font lacks come from system fallback fonts.
    #[test]
    fn fallback_fonts_cover_more_scripts() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(Default::default(), |_| {});
        let _ = ctx.run(Default::default(), |ctx| {
            #[cfg(target_os = "macos")]
            {
                let fonts = ctx.fonts(|f| f.definitions().clone());
                assert_eq!(fonts.families[&egui::FontFamily::Proportional][0], "sf", "SF must be the default text font");
                assert_eq!(fonts.families[&egui::FontFamily::Monospace][0], "sf_mono", "SF Mono must be the default code font");
            }
            for c in ['ə', 'Ə', 'ş', 'ğ', 'Ж', 'λ', '→', '中'] {
                for font in [egui::FontId::proportional(14.0), egui::FontId::monospace(13.0)] {
                    assert!(ctx.fonts_mut(|f| f.has_glyph(&font, c)), "missing {c:?} in {font:?}");
                }
            }
        });
    }

    #[test]
    fn scans_rich_blocks() {
        let d = doc::doc_for(SAMPLE);
        let kinds: Vec<&str> = d
            .rich
            .iter()
            .map(|b| match &b.kind {
                doc::RichKind::Images(_) => "images",
                doc::RichKind::Mermaid(_) => "mermaid",
                doc::RichKind::Math(_) => "math",
                doc::RichKind::Table { .. } => "table",
                doc::RichKind::Hidden => "hidden",
            })
            .collect();
        for k in ["images", "mermaid", "math", "table", "hidden"] {
            assert!(kinds.contains(&k), "missing {k}: {kinds:?}");
        }
        assert!(d.frontmatter.is_some());
        assert_eq!(d.code.len(), 2, "rust fence + indented code");
        assert!(d.quotes.iter().any(|q| q.alert.is_some()));
        assert!(d.quotes.iter().any(|q| q.depth == 2));
        assert_eq!(d.tasks.len(), 3);
        assert!(!d.inline_math.is_empty());
        assert_eq!(d.setext_titles.len(), 1);
        assert!(!d.footnote_defs.is_empty());
    }

    /// The cached layout must match a fresh one, and follow the cursor in and out of rich blocks.
    #[test]
    fn cached_layout_follows_reveal() {
        let ctx = egui::Context::default();
        let env = Env::default();
        let text = "Intro\n\n```mermaid\ngraph TD; A-->B;\n```\n\nOutro";
        let inside = text.find("graph").unwrap();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let hidden = live_layouter(ui, text, 500.0, None, &env);
                let shown = live_layouter(ui, text, 500.0, Some((inside, inside)), &env);
                let again = live_layouter(ui, text, 500.0, None, &env);
                assert_ne!(hidden.size(), shown.size(), "cursor inside a diagram shows its source");
                assert_eq!(hidden.job.sections.len(), again.job.sections.len());
                assert_eq!(hidden.size(), again.size());
                // A cursor in plain text reuses the cached job.
                let plain = live_layouter(ui, text, 500.0, Some((1, 1)), &env);
                assert_eq!(hidden.size(), plain.size());
            });
        });
    }

    #[test]
    fn code_block_lines_map_to_body_lines() {
        let t = "```rust\nlet a = 1;\nlet b = 2;\n```\n\n```\nunclosed\nstill";
        let d = doc::doc_for(t);
        let cb = &d.code[0];
        assert_eq!(&t[cb.body.clone()], "let a = 1;\nlet b = 2;");
        assert_eq!(cb.body_line(t.find("let a").unwrap()), Some(0));
        assert_eq!(cb.body_line(t.find("let b").unwrap()), Some(1));
        assert_eq!(cb.body_line(0), None, "opening fence");
        assert_eq!(cb.highlight(t).map(|h| h.len()), Some(2));
        let open = &d.code[1];
        assert_eq!(&t[open.body.clone()], "unclosed\nstill");
        assert_eq!(open.body_line(t.find("still").unwrap()), Some(1));
    }

    #[test]
    fn renders_mermaid_and_math() {
        let ctx = egui::Context::default();
        assert!(visual::mermaid(&ctx, "graph TD; A-->B;").is_ok());
        assert!(visual::math(&ctx, r"\frac{1}{2} + \sqrt{x}", true).is_ok());
    }

    #[test]
    fn clicks() {
        let t = "text\n- [ ] task\nsee [[Other|o]] and [w](https://x.y) [h](#text-top)\n# Text top";
        assert!(matches!(click_action(t, 7, false), Some(PreviewAction::ToggleTask(r)) if &t[r.clone()] == "[ ]"));
        let wiki = t.find("Other").unwrap();
        assert!(matches!(click_action(t, wiki, true), Some(PreviewAction::OpenNote(n)) if n == "Other"));
        let web = t.find("https").unwrap();
        assert!(matches!(click_action(t, web, true), Some(PreviewAction::OpenUrl(u)) if u == "https://x.y"));
        let anchor = t.find("[h]").unwrap();
        assert!(matches!(click_action(t, anchor, true), Some(PreviewAction::Jump(_))));
        assert!(click_action(t, wiki, false).is_none());
    }
}

#[cfg(test)]
mod visual_check {
    /// Rasterizes a diagram and a formula through egui's real image loaders into PNGs
    /// (in $VAULT_SNAPSHOT_DIR) so they can be eyeballed. Run with `--ignored`.
    #[test]
    #[ignore]
    fn snapshot_svgs() {
        let dir = std::env::var("VAULT_SNAPSHOT_DIR").expect("set VAULT_SNAPSHOT_DIR");
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let items = [
            ("mermaid", super::visual::mermaid(&ctx, "flowchart LR\n  A[Start] --> B{Decision}\n  B -->|Yes| C[Done]\n  B -->|No| D[Retry]").unwrap()),
            ("pie", super::visual::mermaid(&ctx, "pie title Pets\n  \"Dogs\" : 386\n  \"Cats\" : 85").unwrap()),
            ("math", super::visual::math(&ctx, r"x = \frac{-b \pm \sqrt{b^2-4ac}}{2a}", true).unwrap()),
        ];
        for (name, v) in items {
            let mut img = None;
            for _ in 0..50 {
                match ctx.try_load_image(&v.uri, egui::SizeHint::Scale(2.0.into())) {
                    Ok(egui::load::ImagePoll::Ready { image }) => {
                        img = Some(image);
                        break;
                    }
                    _ => std::thread::sleep(std::time::Duration::from_millis(50)),
                }
            }
            let img = img.expect("image never loaded");
            let [w, h] = img.size;
            let mut buf = image::RgbaImage::new(w as u32, h as u32);
            for (i, px) in img.pixels.iter().enumerate() {
                // Composite onto the app's dark background.
                let bg = [18u8, 18, 20];
                let a = px.a() as u32;
                let mix = |c: u8, b: u8| ((c as u32 * 255 + b as u32 * (255 - a)) / 255).min(255) as u8;
                let p = image::Rgba([mix(px.r(), bg[0]), mix(px.g(), bg[1]), mix(px.b(), bg[2]), 255]);
                buf.put_pixel((i % w) as u32, (i / w) as u32, p);
            }
            buf.save(format!("{dir}/{name}.png")).unwrap();
            println!("{name}: natural {:?}, raster {w}x{h}", v.size);
        }
    }
}

