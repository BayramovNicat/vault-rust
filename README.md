# Vault (Pure Rust Native)

High-performance, pure native Markdown viewer and personal knowledge base built with Rust, `egui`, and `pulldown-cmark`.

- **No Webview / No Browser Engine**: Pure native OpenGL/Metal hardware acceleration.
- **Instant Bootup**: Window displays in **~25–35 ms**.
- **Ultra-low Memory**: Consumes only **~28 MB RAM** (vs 80–120 MB in WebKit).
- **Zero Idle CPU**: Event-driven repainting maintains strictly **0.0% CPU** when idle.
- **Single Lightweight Binary**: **5.6 MB** release binary.

---

## Features

- **Minimal window**: just the notes sidebar and the note on a blurred glass background (macOS). Custom title strip with the window controls and the note's name; drag it to move the window, double-click to zoom. San Francisco (the macOS system font) throughout.
- **Notes Sidebar**:
  - Live search filter as you type.
  - Folder shown as a faint suffix for nested notes.
  - Right-click a note to rename, copy its name or delete it.
  - Toggle with the sidebar button next to the window controls, or `Cmd+B`.
  - New note with `+` or `Cmd+N`.
- **Edit / Preview toggle**: one small button in the top-right corner (`Cmd+E`). A dot next to it means changes are about to autosave.
- **Markdown Viewer & Editor**:
  - **Live Preview Mode** (edit in place, Markdown symbols hidden — like Obsidian/Typora):
    - Headings (ATX + setext), bold, italics, strikethrough, `==highlight==`, inline code, escapes.
    - Lists (nested), ordered lists, clickable task checkboxes, definition lists.
    - Blockquotes (nested) and GitHub alerts (`> [!NOTE]`, TIP, IMPORTANT, WARNING, CAUTION).
    - Tables with column alignment, edited in place: click a cell, Tab / Shift+Tab / Enter to move, right-click to add or delete rows and columns.
    - Code blocks with syntax highlighting and a "Copy" button.
    - Images: local paths, web (`https://`), HTML `<img>`, Obsidian `![[image.png]]`, README badges. PNG/JPEG/GIF/WebP/SVG.
    - Mermaid diagrams (flowchart, sequence, class, state, ER, pie, gantt, xychart, mindmap, …) — pure Rust, offline.
    - LaTeX math: inline `$...$` and display `$$...$$` / ```` ```math ````.
    - Links, bare URLs, `[[Wiki-links]]`, heading anchors, footnotes, emoji shortcodes (`:rocket:`), inline HTML (`<kbd>`, `<sub>`, `<sup>`, `<mark>`, `<details>`), YAML frontmatter.
    - Click into an image, diagram, table or formula to edit its source; ⌘+click opens links.
    - See [`docs/showcase.md`](docs/showcase.md) for every format (`VAULT_DIR=docs cargo run --release`).
  - **Source Editor Mode (`Cmd+E`)**:
    - Fast multiline code editor with monospace font and tab indentation.
    - Background debounced autosave (~400ms after typing).
- **Find in Note (`Cmd+F`)**:
  - Floating search bar with a match counter; Enter / Shift+Enter (or ↓ / ↑) jump between matches and select them.
- **Linked Mentions (Backlinks)**:
  - Automatically calculates which notes reference the current note.
  - Shows line numbers, context excerpts, and one-click navigation.
- **File System Watcher**:
  - Real-time background file watching (`notify` crate) automatically updates the file tree when external changes occur.
- **Configurable Notes Folder**:
  - Default: `~/.vault`
  - Override via environment variable: `VAULT_DIR=/path/to/my/vault`

---

## Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| `Cmd + S` | Manual Save |
| `Cmd + N` | New Note Dialog |
| `Cmd + F` | Find in Active Note |
| `Cmd + E` | Toggle Preview / Edit (raw Markdown) |
| `Cmd + B` / `Cmd + \` | Toggle Sidebar |
| `Escape` | Close Find / Dismiss Dialogs |

---

## Build & Run

### Run directly
```sh
cargo run --release
```

### Build optimized release binary
```sh
cargo build --release
```
The binary will be generated at `./target/release/vault` (5.6 MB).

### Build the macOS app (with icon)
```sh
./bundle.sh
```
Creates `Vault.app` with the icon from `assets/icon.svg`. Drag it into Applications and open it like any other app (no Terminal window). Run the script again after code changes to update it.
