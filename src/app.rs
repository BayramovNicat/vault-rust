use std::time::{Duration, Instant, SystemTime};

use egui::{
    text::{LayoutJob, TextWrapping},
    Align, Align2, Color32, FontId, Layout, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, TextFormat, Vec2,
};

use crate::markdown::{click_action, doc_for, live_layouter, paint_overlay, toggle_details, Env, PreviewAction, SvgByContentLoader};
use crate::theme::colors;
use crate::vault::{MarkdownFile, VaultStore};
use crate::wiki_links::Backlink;

/// Height of the strip at the top of the window that holds the macOS traffic lights.
const TITLE_STRIP: f32 = 38.0;
/// Max width of the note column (readable line length).
const COLUMN_MAX: f32 = 760.0;
const PREVIEW_ID: &str = "live_preview_editor";
const SOURCE_ID: &str = "source_editor";

pub struct VaultApp {
    pub store: VaultStore,
    pub selected_path: Option<String>,
    pub content: String,
    pub is_dirty: bool,
    pub last_edit_time: Instant,
    /// mtime of the open note when we last read or wrote it (detects edits made outside the app).
    loaded_mtime: Option<SystemTime>,

    // View
    pub sidebar_collapsed: bool,
    pub search_query: String,
    pub is_source_mode: bool,
    pub outline_open: bool,

    // Navigation history
    nav_back: Vec<String>,
    nav_forward: Vec<String>,

    // Quick Switcher (Cmd+P / Cmd+K)
    quick_open: bool,
    quick_query: String,
    quick_index: usize,
    quick_focus: bool,

    // Wiki-link autocomplete
    ac_index: usize,
    ac_dismissed_at: Option<usize>,

    // Find in note
    pub find_open: bool,
    pub find_query: String,
    /// Byte ranges of the matches in `content`.
    pub find_matches: Vec<(usize, usize)>,
    /// The same matches as char ranges (what the editor's cursors use).
    find_match_chars: Vec<(usize, usize)>,
    pub find_match_index: usize,
    /// Byte range to select (and scroll to) in the editor on the next frame.
    pending_select: Option<(usize, usize)>,
    /// Focus the find field on the next frame (set when the bar opens).
    find_focus: bool,

    // Dialogs
    pub show_create_dialog: bool,
    pub new_note_input: String,
    pub show_rename_dialog: bool,
    pub rename_input: String,
    pub show_delete_dialog: bool,

    /// Short-lived error message (failed save, bad name, ...).
    error: Option<(String, Instant)>,
    /// The window has the macOS blurred-glass background.
    glass: bool,

    pub cached_backlinks: Vec<Backlink>,
}

impl VaultApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Image loaders for the preview: local files, web images, PNG/JPEG/GIF/WebP/SVG.
        egui_extras::install_image_loaders(&cc.egui_ctx);
        // Added last so it's tried first: web SVGs without a `.svg` extension (README badges).
        cc.egui_ctx.add_image_loader(std::sync::Arc::new(SvgByContentLoader::default()));
        // Wake the UI when files change on disk, so the sidebar updates without user input.
        let ctx = cc.egui_ctx.clone();
        let store = VaultStore::new(move || ctx.request_repaint()).expect("Failed to initialize VaultStore");
        let mut app = Self {
            store,
            selected_path: None,
            content: String::new(),
            is_dirty: false,
            last_edit_time: Instant::now(),
            loaded_mtime: None,
            sidebar_collapsed: false,
            search_query: String::new(),
            is_source_mode: false,
            outline_open: false,
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            quick_open: false,
            quick_query: String::new(),
            quick_index: 0,
            quick_focus: false,
            ac_index: 0,
            ac_dismissed_at: None,
            find_open: false,
            find_query: String::new(),
            find_matches: Vec::new(),
            find_match_chars: Vec::new(),
            find_match_index: 0,
            pending_select: None,
            find_focus: false,
            show_create_dialog: false,
            new_note_input: String::new(),
            show_rename_dialog: false,
            rename_input: String::new(),
            show_delete_dialog: false,
            error: None,
            glass: apply_glass(cc),
            cached_backlinks: Vec::new(),
        };

        let restored = app.load_session();
        if let Some(path) = restored.filter(|p| app.store.file(p).is_some()) {
            app.load_note_with_history(&path, false);
        } else if let Some(first) = app.store.files.first() {
            let path = first.relative_path.clone();
            app.load_note_with_history(&path, false);
        }

        app
    }

    fn show_error(&mut self, msg: String) {
        self.error = Some((msg, Instant::now()));
    }

    pub fn load_note(&mut self, path: &str) {
        self.load_note_with_history(path, true);
    }

    pub fn load_note_with_history(&mut self, path: &str, record_history: bool) {
        if self.selected_path.as_deref() == Some(path) {
            return;
        }
        if self.is_dirty {
            self.save_current();
        }

        match self.store.read_file(path) {
            Ok(content) => {
                if record_history {
                    if let Some(prev) = self.selected_path.take() {
                        self.nav_back.push(prev);
                        if self.nav_back.len() > 64 {
                            self.nav_back.remove(0);
                        }
                        self.nav_forward.clear();
                    }
                }
                self.selected_path = Some(path.to_string());
                self.content = content;
                self.is_dirty = false;
                self.loaded_mtime = self.store.file(path).map(|f| f.mtime);
                self.ac_dismissed_at = None;
                self.refresh_backlinks();
                self.update_find_matches();
            }
            Err(e) => self.show_error(format!("Couldn't open {path}: {e}")),
        }
    }

    fn go_back(&mut self) {
        while let Some(prev) = self.nav_back.pop() {
            if self.store.file(&prev).is_some() {
                if let Some(cur) = self.selected_path.clone() {
                    self.nav_forward.push(cur);
                }
                self.load_note_with_history(&prev, false);
                return;
            }
        }
    }

    fn go_forward(&mut self) {
        while let Some(next) = self.nav_forward.pop() {
            if self.store.file(&next).is_some() {
                if let Some(cur) = self.selected_path.clone() {
                    self.nav_back.push(cur);
                }
                self.load_note_with_history(&next, false);
                return;
            }
        }
    }

    fn session_path(&self) -> std::path::PathBuf {
        self.store.root.join(".vault_state")
    }

    fn load_session(&mut self) -> Option<String> {
        let raw = std::fs::read_to_string(self.session_path()).ok()?;
        let mut last_note = None;
        for line in raw.lines() {
            if let Some(val) = line.strip_prefix("last_note=") {
                if !val.is_empty() {
                    last_note = Some(val.to_string());
                }
            } else if let Some(val) = line.strip_prefix("sidebar_collapsed=") {
                self.sidebar_collapsed = val == "true";
            }
        }
        last_note
    }

    fn save_session(&self) {
        let note = self.selected_path.as_deref().unwrap_or("");
        let data = format!("last_note={note}\nsidebar_collapsed={}\n", self.sidebar_collapsed);
        let _ = std::fs::write(self.session_path(), data);
    }

    pub fn save_current(&mut self) {
        if let Some(path) = self.selected_path.clone() {
            match self.store.write_file(&path, &self.content) {
                Ok(_) => {
                    self.is_dirty = false;
                    self.loaded_mtime = self.store.file(&path).map(|f| f.mtime);
                    self.refresh_backlinks();
                }
                Err(e) => self.show_error(format!("Couldn't save: {e}")),
            }
        }
    }

    pub fn refresh_backlinks(&mut self) {
        if let Some(ref path) = self.selected_path {
            self.cached_backlinks = self.store.resolver().find_backlinks(path, &self.store.link_index);
        } else {
            self.cached_backlinks.clear();
        }
    }

    /// Recomputes find matches. Skipped while the find bar is closed (it recomputes on open).
    pub fn update_find_matches(&mut self) {
        self.find_matches.clear();
        self.find_match_chars.clear();
        if !self.find_open || self.find_query.is_empty() {
            return;
        }

        self.find_matches = find_case_insensitive(&self.content, &self.find_query);
        self.find_match_chars = byte_ranges_to_chars(&self.content, &self.find_matches);

        if self.find_match_index >= self.find_matches.len() {
            self.find_match_index = 0;
        }
    }

    /// Selects the current find match in the editor.
    fn select_current_match(&mut self) {
        if let Some(&range) = self.find_matches.get(self.find_match_index) {
            self.pending_select = Some(range);
        }
    }

    fn step_match(&mut self, forward: bool) {
        let n = self.find_matches.len();
        if n == 0 {
            return;
        }
        self.find_match_index = if forward { (self.find_match_index + 1) % n } else { (self.find_match_index + n - 1) % n };
        self.select_current_match();
    }

    fn open_create_dialog(&mut self) {
        self.show_create_dialog = true;
        self.new_note_input.clear();
    }

    fn open_rename_dialog(&mut self, file: &MarkdownFile) {
        if self.selected_path.as_deref() != Some(file.relative_path.as_str()) {
            self.load_note(&file.relative_path);
        }
        self.rename_input = file.title().to_string();
        self.show_rename_dialog = true;
    }

    fn active_editor_id(&self, ctx: &egui::Context) -> Option<egui::Id> {
        let id = egui::Id::new(if self.is_source_mode { SOURCE_ID } else { PREVIEW_ID });
        ctx.memory(|m| m.has_focus(id)).then_some(id)
    }

    pub fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = egui::Modifiers::COMMAND;
        let cmd_shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        let editor_focused = self.active_editor_id(ctx).is_some();

        if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::S)) {
            self.save_current();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::N)) {
            self.open_create_dialog();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::P) || (!editor_focused && i.consume_key(cmd, egui::Key::K))) {
            self.quick_open = !self.quick_open;
            if self.quick_open {
                self.quick_query.clear();
                self.quick_index = 0;
                self.quick_focus = true;
            }
        }
        if ctx.input_mut(|i| i.consume_key(cmd_shift, egui::Key::O)) {
            self.outline_open = !self.outline_open;
        }
        if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::OpenBracket)) {
            self.go_back();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::CloseBracket)) {
            self.go_forward();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::F)) {
            self.find_open = !self.find_open;
            self.find_focus = self.find_open;
            self.update_find_matches();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::E)) {
            self.is_source_mode = !self.is_source_mode;
        }
        if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::Backslash) || i.consume_key(cmd_shift, egui::Key::B) || (!editor_focused && i.consume_key(cmd, egui::Key::B))) {
            self.sidebar_collapsed = !self.sidebar_collapsed;
            self.save_session();
        }
        if let Some(ed_id) = self.active_editor_id(ctx) {
            if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::B)) {
                self.wrap_selection(ctx, ed_id, "**", "**");
            }
            if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::I)) {
                self.wrap_selection(ctx, ed_id, "*", "*");
            }
            if ctx.input_mut(|i| i.consume_key(cmd, egui::Key::K)) {
                self.wrap_selection(ctx, ed_id, "[", "](url)");
            }
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.find_open = false;
            self.quick_open = false;
            self.outline_open = false;
            self.show_create_dialog = false;
            self.show_rename_dialog = false;
            self.show_delete_dialog = false;
        }
    }

    fn wrap_selection(&mut self, ctx: &egui::Context, id: egui::Id, prefix: &str, suffix: &str) {
        let Some(mut state) = egui::TextEdit::load_state(ctx, id) else { return };
        let Some(range) = state.cursor.char_range() else { return };
        let (a, b) = char_range_to_bytes(&self.content, range.primary.index, range.secondary.index);
        let selected = self.content[a..b].to_string();
        let replacement = format!("{prefix}{selected}{suffix}");
        self.content.replace_range(a..b, &replacement);
        self.mark_edited();
        let new_start = self.content[..a].chars().count() + prefix.chars().count();
        let new_end = new_start + selected.chars().count();
        let c1 = egui::text::CCursor::new(new_start);
        let c2 = egui::text::CCursor::new(new_end);
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(c1, c2)));
        state.store(ctx, id);
        ctx.request_repaint();
    }

    pub fn poll_watcher(&mut self) {
        if !self.store.take_disk_changes() {
            return;
        }
        self.store.refresh_files();
        // The open note was changed by another app: show the new version, unless we have unsaved edits.
        if let Some(path) = self.selected_path.clone() {
            let on_disk = self.store.file(&path).map(|f| f.mtime);
            if !self.is_dirty && on_disk.is_some() && on_disk != self.loaded_mtime {
                if let Ok(content) = self.store.read_file(&path) {
                    self.content = content;
                    self.loaded_mtime = on_disk;
                    self.update_find_matches();
                }
            }
        }
        self.refresh_backlinks();
    }

    /// Flips a task marker (`[ ]` <-> `[x]`) at the given byte range of the note.
    fn toggle_task(&mut self, range: std::ops::Range<usize>) {
        let Some(marker) = self.content.get(range.clone()) else { return };
        let replacement = if marker.contains(' ') { "[x]" } else { "[ ]" };
        self.content.replace_range(range, replacement);
        self.mark_edited();
    }

    fn mark_edited(&mut self) {
        self.is_dirty = true;
        self.last_edit_time = Instant::now();
        self.update_find_matches();
    }

    /// Folders used to find images referenced from the current note.
    fn preview_env(&self) -> Env {
        let note_dir = self
            .selected_path
            .as_ref()
            .and_then(|p| self.store.root.join(p).parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| self.store.root.clone());
        Env { note_dir, vault_root: self.store.root.clone() }
    }

    fn open_link_target(&mut self, target: &str) {
        let (note_part, heading_part) = match target.split_once('#') {
            Some((n, h)) => (n.trim(), Some(h.trim())),
            None => (target.trim(), None),
        };
        if !note_part.is_empty() {
            if let Some(resolved_path) = self.store.resolver().resolve(note_part) {
                self.load_note(&resolved_path);
            } else {
                let new_filename = VaultStore::normalize_filename(note_part);
                match self.store.create_file(&new_filename, &format!("# {note_part}\n\n")) {
                    Ok(path) => self.load_note(&path),
                    Err(e) => {
                        self.show_error(format!("Couldn't create {new_filename}: {e}"));
                        return;
                    }
                }
            }
        }
        if let Some(anchor) = heading_part.filter(|h| !h.is_empty()) {
            let slug = crate::markdown::doc_for(&self.content)
                .headings
                .iter()
                .find(|(_, _, s)| s == &slugify_anchor(anchor))
                .map(|(at, _, _)| *at);
            if let Some(byte) = slug {
                self.pending_select = Some((byte, byte));
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        for f in dropped {
            let Some(path) = f.path else { continue };
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if ext == "md" {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("imported.md");
                    match self.store.create_file(name, &text) {
                        Ok(rel) => self.load_note(&rel),
                        Err(e) => self.show_error(format!("Couldn't import {name}: {e}")),
                    }
                }
            } else if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg") {
                if self.selected_path.is_some() {
                    match self.store.import_asset(&path) {
                        Ok(rel) => {
                            let alt = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
                            if !self.content.is_empty() && !self.content.ends_with('\n') {
                                self.content.push('\n');
                            }
                            self.content.push_str(&format!("![{alt}]({rel})\n"));
                            self.mark_edited();
                        }
                        Err(e) => self.show_error(format!("Couldn't import image: {e}")),
                    }
                }
            }
        }
    }
}

/// Makes an empty strip draggable like a title bar (double-click zooms the window).
fn title_drag_strip(ui: &mut egui::Ui, rect: Rect, id: &str) {
    let resp = ui.interact(rect, egui::Id::new(id), Sense::click_and_drag());
    if resp.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
}

impl eframe::App for VaultApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if self.glass { [0.0; 4] } else { colors::BG_APP.to_normalized_gamma_f32() }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_watcher();
        self.handle_dropped_files(ctx);
        self.handle_shortcuts(ctx);

        // Autosave shortly after typing stops.
        if self.is_dirty && self.last_edit_time.elapsed() > Duration::from_millis(400) {
            self.save_current();
        }
        if self.is_dirty {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if !self.sidebar_collapsed {
            self.sidebar(ctx);
        }
        self.note_area(ctx);
        self.floating_controls(ctx);
        self.quick_switcher(ctx);
        self.error_toast(ctx);
        self.render_dialogs(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.is_dirty {
            self.save_current();
        }
        self.save_session();
    }
}

impl VaultApp {
    // -----------------------------------------------------------------------
    // Sidebar
    // -----------------------------------------------------------------------

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(250.0)
            .width_range(180.0..=420.0)
            .frame(
                egui::Frame::new()
                    .fill(if self.glass { colors::GLASS_SIDEBAR } else { colors::BG_SIDEBAR })
                    .inner_margin(egui::Margin { left: 10, right: 10, top: 0, bottom: 10 })
                    .stroke(Stroke::new(1.0_f32, Color32::from_white_alpha(8))),
            )
            .show(ctx, |ui| {
                // Title strip (traffic lights live here) doubles as a window drag handle.
                let strip = Rect::from_min_size(ui.max_rect().min - Vec2::new(10.0, 0.0), Vec2::new(ui.max_rect().width() + 20.0, TITLE_STRIP));
                title_drag_strip(ui, strip, "sidebar_drag");
                ui.add_space(TITLE_STRIP + 4.0);

                // Header: "Notes" + count, new-note button.
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Notes").size(13.0).color(colors::TEXT_PRIMARY));
                    ui.label(RichText::new(self.store.files.len().to_string()).size(11.5).color(colors::TEXT_MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, Icon::Plus).on_hover_text("New note  ⌘N").clicked() {
                            self.open_create_dialog();
                        }
                    });
                });
                ui.add_space(8.0);

                // Search field
                let input_fill = if self.glass { colors::GLASS_INPUT } else { colors::BG_INPUT };
                egui::Frame::new().fill(input_fill).corner_radius(7.0).inner_margin(egui::Margin { left: 6, right: 4, top: 3, bottom: 3 }).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                        paint_icon(ui.painter(), r, Icon::Search, colors::TEXT_MUTED);
                        // Exact width: the gap before the field (4) + the clear button and its gap (22 + 4).
                        // Overshooting even slightly makes the resizable sidebar grow every frame.
                        let clear_w = if self.search_query.is_empty() { 0.0 } else { 26.0 };
                        let field_w = (ui.available_width() - 4.0 - clear_w).max(20.0);
                        ui.add_sized(
                            Vec2::new(field_w, 22.0),
                            egui::TextEdit::singleline(&mut self.search_query)
                                .frame(false)
                                .hint_text(RichText::new("Search").color(colors::TEXT_MUTED))
                                .desired_width(field_w)
                                .font(FontId::proportional(13.0))
                                .margin(egui::Margin::symmetric(0, 3)),
                        );
                        if !self.search_query.is_empty() && icon_button_sized(ui, Icon::Close, 22.0).clicked() {
                            self.search_query.clear();
                        }
                    });
                });
                ui.add_space(8.0);

                // File list
                let query = self.search_query.trim().to_lowercase();

                let mut to_open = None;
                let mut to_rename = None;
                let mut to_delete = None;

                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let mut shown = 0;
                    for file in self.store.files.iter().filter(|f| {
                        if query.is_empty() {
                            return true;
                        }
                        if f.search_key.contains(&query) {
                            return true;
                        }
                        if let Some(meta) = self.store.meta_index.get(&f.relative_path) {
                            let tag_q = query.strip_prefix('#').unwrap_or(&query);
                            return meta.tags.iter().any(|t| t.contains(tag_q)) || meta.content_lower.contains(&query);
                        }
                        false
                    }) {
                        shown += 1;
                        let selected = self.selected_path.as_deref() == Some(file.relative_path.as_str());
                        let content_match = !query.is_empty()
                            && !file.search_key.contains(&query)
                            && self
                                .store
                                .meta_index
                                .get(&file.relative_path)
                                .is_some_and(|m| m.content_lower.contains(&query));
                        let resp = file_row(ui, file, selected, content_match);
                        if resp.clicked() {
                            to_open = Some(file.relative_path.clone());
                        }
                        resp.context_menu(|ui| {
                            ui.set_min_width(140.0);
                            if ui.button("Rename").clicked() {
                                to_rename = Some(file.clone());
                                ui.close();
                            }
                            if ui.button("Copy name").clicked() {
                                ui.ctx().copy_text(file.title().to_string());
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(RichText::new("Delete").color(colors::DANGER)).clicked() {
                                to_delete = Some(file.relative_path.clone());
                                ui.close();
                            }
                        });
                    }
                    if shown == 0 {
                        ui.add_space(16.0);
                        ui.vertical_centered(|ui| {
                            let msg = if query.is_empty() { "No notes yet" } else { "No matches" };
                            ui.label(RichText::new(msg).size(12.5).color(colors::TEXT_MUTED));
                        });
                    }
                });

                if let Some(path) = to_open {
                    self.load_note(&path);
                }
                if let Some(file) = to_rename {
                    self.open_rename_dialog(&file);
                }
                if let Some(path) = to_delete {
                    if self.selected_path.as_deref() != Some(path.as_str()) {
                        self.load_note(&path);
                    }
                    self.show_delete_dialog = true;
                }
            });
    }

    // -----------------------------------------------------------------------
    // Note area
    // -----------------------------------------------------------------------

    fn note_area(&mut self, ctx: &egui::Context) {
        let fill = if self.glass { colors::GLASS_APP } else { colors::BG_APP };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(fill)).show(ctx, |ui| {
            let full = ui.max_rect();
            let strip = Rect::from_min_size(full.min, Vec2::new(full.width(), TITLE_STRIP));
            title_drag_strip(ui, strip, "note_drag");

            // Custom title: the note's name, centered in the title strip.
            if let Some(path) = &self.selected_path {
                let title = path.rsplit('/').next().unwrap_or(path);
                let title = title.strip_suffix(".md").unwrap_or(title);
                ui.painter().text(strip.center(), Align2::CENTER_CENTER, title, FontId::proportional(12.5), colors::TEXT_SECONDARY);
            }

            if self.selected_path.is_none() {
                ui.scope_builder(egui::UiBuilder::new().max_rect(full), |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new("No note selected\n\n⌘N to create one").size(14.0).color(colors::TEXT_MUTED),
                        );
                    });
                });
                return;
            }

            let mut link_to_open: Option<String> = None;
            let editor_min_h = (full.height() - TITLE_STRIP - 120.0).max(200.0);

            // The note scrolls only below the title strip, so text never runs under the title.
            let body = Rect::from_min_max(Pos2::new(full.left(), strip.bottom()), full.max);
            let scrolled = ui
                .scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                    ui.set_clip_rect(body);
                    ScrollArea::vertical()
                        .id_salt("note_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let avail = ui.available_width();
                            let col = (avail - 64.0).clamp(200.0, COLUMN_MAX);
                            let pad = ((avail - col) / 2.0).max(0.0);

                            ui.add_space(14.0);
                            ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                                ui.add_space(pad);
                                ui.vertical(|ui| {
                                    ui.set_width(col);
                                    if self.is_source_mode {
                                        self.source_editor(ui, editor_min_h);
                                    } else if let Some(target) = self.preview_editor(ui, editor_min_h) {
                                        link_to_open = Some(target);
                                    }
                                    if let Some(target) = self.backlinks(ui) {
                                        link_to_open = Some(target);
                                    }
                                    ui.add_space(120.0);
                                });
                            });
                        })
                        .state
                        .offset
                        .y
                        > 0.5
                })
                .inner;

            // Thin divider under the title once the note is scrolled.
            if scrolled {
                ui.painter().line_segment(
                    [Pos2::new(full.left(), strip.bottom()), Pos2::new(full.right(), strip.bottom())],
                    Stroke::new(1.0_f32, Color32::from_white_alpha(10)),
                );
            }

            // Quiet word count & reading time in the bottom-right corner.
            let words = self.content.split_whitespace().count();
            let mins = (words / 200).max(1);
            let stats = if words == 1 { "1 word".to_string() } else { format!("{words} words · {mins} min") };
            ui.painter().text(
                Pos2::new(full.right() - 16.0, full.bottom() - 12.0),
                Align2::RIGHT_BOTTOM,
                stats,
                FontId::proportional(11.0),
                colors::TEXT_MUTED,
            );

            if let Some(target) = link_to_open {
                self.open_link_target(&target);
            }
        });
    }

    /// Applies a pending find selection to an editor and scrolls to it.
    fn apply_pending_select(&mut self, ui: &egui::Ui, id: egui::Id, output: &egui::text_edit::TextEditOutput) {
        let Some((a, b)) = self.pending_select.take() else { return };
        let ca = self.content[..a.min(self.content.len())].chars().count();
        let cb = self.content[..b.min(self.content.len())].chars().count();
        let mut state = output.state.clone();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(ca), egui::text::CCursor::new(cb))));
        state.store(ui.ctx(), id);
        let rect = output.galley.pos_from_cursor(egui::text::CCursor::new(ca)).translate(output.galley_pos.to_vec2());
        ui.scroll_to_rect(rect.expand(60.0), Some(Align::Center));
        ui.ctx().request_repaint();
    }

    /// Paints find matches behind the text (current match stronger).
    fn paint_find_highlights(&self, ui: &egui::Ui, output: &egui::text_edit::TextEditOutput, slot: egui::layers::ShapeIdx) {
        if !self.find_open || self.find_matches.is_empty() {
            return;
        }
        let g = &output.galley;
        let off = output.galley_pos.to_vec2();
        let clip = ui.clip_rect().expand(100.0);
        let mut shapes = Vec::new();
        for (n, &(ca, cb)) in self.find_match_chars.iter().enumerate() {
            let a = g.pos_from_cursor(egui::text::CCursor::new(ca)).translate(off);
            if a.top() > clip.bottom() {
                break; // Matches are in order: the rest are below the view.
            }
            let b = g.pos_from_cursor(egui::text::CCursor::new(cb)).translate(off);
            if b.bottom() < clip.top() {
                continue;
            }
            let color = if n == self.find_match_index {
                Color32::from_rgba_unmultiplied(235, 175, 40, 150)
            } else {
                Color32::from_rgba_unmultiplied(235, 175, 40, 55)
            };
            let rects: Vec<Rect> = if (a.top() - b.top()).abs() < 0.5 {
                vec![Rect::from_min_max(a.left_top(), Pos2::new(b.left(), a.bottom()))]
            } else {
                g.rows
                    .iter()
                    .filter_map(|row| {
                        let rr = row.rect().translate(off);
                        if rr.bottom() < a.top() + 0.5 || rr.top() > b.bottom() - 0.5 {
                            return None;
                        }
                        let x0 = if (rr.top() - a.top()).abs() < 0.5 { a.left() } else { rr.left() };
                        let x1 = if (rr.top() - b.top()).abs() < 0.5 { b.left() } else { rr.right() };
                        Some(Rect::from_min_max(Pos2::new(x0, rr.top()), Pos2::new(x1, rr.bottom())))
                    })
                    .collect()
            };
            for r in rects {
                if r.width() > 0.5 {
                    shapes.push(egui::Shape::rect_filled(r.expand2(Vec2::new(1.0, 0.0)), 3.0, color));
                }
            }
        }
        ui.painter().set(slot, egui::Shape::Vec(shapes));
    }

    fn source_editor(&mut self, ui: &mut egui::Ui, min_h: f32) {
        let id = egui::Id::new(SOURCE_ID);
        let prev_len = self.content.len();
        let hl_slot = ui.painter().add(egui::Shape::Noop);
        let output = egui::TextEdit::multiline(&mut self.content)
            .id(id)
            .font(FontId::monospace(13.5))
            .text_color(colors::TEXT_PRIMARY)
            .frame(false)
            .lock_focus(true)
            .desired_width(f32::INFINITY)
            .min_size(Vec2::new(0.0, min_h))
            .show(ui);
        if output.response.changed() {
            self.mark_edited();
            self.smart_enter_continuation(ui.ctx(), id, &output, prev_len);
        }
        self.wiki_autocomplete(ui.ctx(), id, &output);
        self.paint_find_highlights(ui, &output, hl_slot);
        self.apply_pending_select(ui, id, &output);
    }

    /// Live preview: editable, with Markdown markers hidden. Rich blocks (images, diagrams,
    /// tables, math) render, and show their source only while the cursor is inside them.
    /// Returns a note/link target to open.
    fn preview_editor(&mut self, ui: &mut egui::Ui, min_h: f32) -> Option<String> {
        let edit_id = egui::Id::new(PREVIEW_ID);
        let reveal = if ui.memory(|m| m.has_focus(edit_id)) {
            egui::TextEdit::load_state(ui.ctx(), edit_id)
                .and_then(|st| st.cursor.char_range())
                .map(|r| char_range_to_bytes(&self.content, r.primary.index, r.secondary.index))
        } else {
            None
        };
        let env = self.preview_env();

        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
            live_layouter(ui, buffer.as_str(), wrap_width, reveal, &env)
        };

        // Reserved now, filled after the editor is drawn, so backgrounds sit under the text.
        let prev_len = self.content.len();
        let bg_slot = ui.painter().add(egui::Shape::Noop);
        let hl_slot = ui.painter().add(egui::Shape::Noop);
        let output = egui::TextEdit::multiline(&mut self.content)
            .id(edit_id)
            .layouter(&mut layouter)
            .lock_focus(true)
            .frame(false)
            .desired_width(f32::INFINITY)
            .min_size(Vec2::new(0.0, min_h))
            .show(ui);

        if output.response.changed() {
            self.mark_edited();
            self.smart_enter_continuation(ui.ctx(), edit_id, &output, prev_len);
        }
        self.wiki_autocomplete(ui.ctx(), edit_id, &output);

        // Cursor moved in/out of a rich block: lay out again right away.
        let new_reveal = if output.response.has_focus() {
            output.cursor_range.map(|r| char_range_to_bytes(&self.content, r.primary.index, r.secondary.index))
        } else {
            None
        };
        if new_reveal != reveal {
            ui.ctx().request_repaint();
        }

        let mut action = paint_overlay(ui, &output, &self.content, new_reveal, &env, bg_slot);
        self.paint_find_highlights(ui, &output, hl_slot);

        // Cmd+hover over a link: pointer cursor.
        if ui.input(|i| i.modifiers.command) {
            if let Some(pos) = output.response.hover_pos() {
                let c = output.galley.cursor_from_pos(pos - output.galley_pos);
                let byte = char_to_byte(&self.content, c.index);
                if matches!(click_action(&self.content, byte, true), Some(PreviewAction::OpenNote(_) | PreviewAction::OpenUrl(_) | PreviewAction::Jump(_))) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        }
        if action.is_none() && output.response.clicked() {
            if let Some(r) = output.cursor_range {
                let pos = char_to_byte(&self.content, r.primary.index);
                let command = ui.input(|i| i.modifiers.command);
                action = click_action(&self.content, pos, command);
            }
        }
        self.apply_pending_select(ui, edit_id, &output);

        match action {
            Some(PreviewAction::OpenNote(target)) => return Some(target),
            Some(PreviewAction::OpenUrl(url)) => open_url(ui.ctx(), &url),
            Some(PreviewAction::ToggleTask(range)) => self.toggle_task(range),
            Some(PreviewAction::ToggleDetails(start)) => {
                toggle_details(ui.ctx(), start);
                ui.ctx().request_repaint();
            }
            Some(PreviewAction::Replace(range, new_text)) => {
                if range.end <= self.content.len() && self.content.is_char_boundary(range.start) && self.content.is_char_boundary(range.end) {
                    self.content.replace_range(range, &new_text);
                    self.mark_edited();
                    ui.ctx().request_repaint();
                }
            }
            Some(PreviewAction::Jump(byte)) => {
                let ci = self.content[..byte.min(self.content.len())].chars().count();
                let cursor = egui::text::CCursor::new(ci);
                let mut state = output.state.clone();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(cursor)));
                state.store(ui.ctx(), edit_id);
                let rect = output.galley.pos_from_cursor(cursor).translate(output.galley_pos.to_vec2());
                ui.scroll_to_rect(rect, Some(Align::TOP));
            }
            None => {}
        }
        None
    }

    fn backlinks(&self, ui: &mut egui::Ui) -> Option<String> {
        if self.cached_backlinks.is_empty() {
            return None;
        }
        let mut open = None;
        ui.add_space(40.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, colors::LINE);
        ui.add_space(14.0);
        ui.label(RichText::new(format!("Linked mentions  {}", self.cached_backlinks.len())).size(12.0).color(colors::TEXT_MUTED));
        ui.add_space(6.0);
        for backlink in &self.cached_backlinks {
            let title = backlink.source_path.strip_suffix(".md").unwrap_or(&backlink.source_path);
            if ui.link(RichText::new(title).size(13.0).color(colors::TEXT_ACCENT)).clicked() {
                open = Some(backlink.source_path.clone());
            }
            for m in backlink.matches.iter().take(3) {
                ui.label(RichText::new(m.line_text.trim()).size(12.0).color(colors::TEXT_MUTED))
                    .on_hover_text(format!("Line {}", m.line_number));
            }
            ui.add_space(6.0);
        }
        open
    }

    // -----------------------------------------------------------------------
    // Floating controls: Edit/Preview toggle, unsaved dot, find bar, sidebar button
    // -----------------------------------------------------------------------

    fn floating_controls(&mut self, ctx: &egui::Context) {
        // Sidebar toggle + Back/Forward history, right of the traffic lights.
        egui::Area::new(egui::Id::new("sidebar_button"))
            .fixed_pos(Pos2::new(80.0, (TITLE_STRIP - 26.0) / 2.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let tip = if self.sidebar_collapsed { "Show sidebar  ⌘\\" } else { "Hide sidebar  ⌘\\" };
                    if icon_button_sized(ui, Icon::Sidebar, 26.0).on_hover_text(tip).clicked() {
                        self.sidebar_collapsed = !self.sidebar_collapsed;
                        self.save_session();
                    }
                    if !self.nav_back.is_empty() && icon_button_sized(ui, Icon::Back, 24.0).on_hover_text("Back  ⌘[").clicked() {
                        self.go_back();
                    }
                    if !self.nav_forward.is_empty() && icon_button_sized(ui, Icon::Forward, 24.0).on_hover_text("Forward  ⌘]").clicked() {
                        self.go_forward();
                    }
                });
            });

        if self.selected_path.is_none() {
            return;
        }

        egui::Area::new(egui::Id::new("mode_toggle"))
            .anchor(Align2::RIGHT_TOP, Vec2::new(-14.0, (TITLE_STRIP - 24.0) / 2.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.is_dirty {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(10.0, 24.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.0, colors::TEXT_MUTED);
                    }
                    if icon_button_sized(ui, Icon::Outline, 24.0).on_hover_text("Outline  ⌘⇧O").clicked() {
                        self.outline_open = !self.outline_open;
                    }
                    let (label, tip) = if self.is_source_mode { ("Preview", "Show preview  ⌘E") } else { ("Edit", "Edit Markdown  ⌘E") };
                    if pill(ui, label, self.is_source_mode).on_hover_text(tip).clicked() {
                        self.is_source_mode = !self.is_source_mode;
                    }
                });
            });

        if self.outline_open {
            let headings = doc_for(&self.content).headings.clone();
            egui::Area::new(egui::Id::new("outline_popover"))
                .anchor(Align2::RIGHT_TOP, Vec2::new(-16.0, TITLE_STRIP + 2.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(colors::BG_CARD)
                        .stroke(Stroke::new(1.0_f32, colors::BORDER_LIGHT))
                        .corner_radius(8.0)
                        .shadow(ctx.style().visuals.popup_shadow)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(240.0);
                            ui.label(RichText::new("Outline").size(12.0).color(colors::TEXT_MUTED));
                            ui.add_space(4.0);
                            if headings.is_empty() {
                                ui.label(RichText::new("No headings").size(12.0).color(colors::TEXT_MUTED));
                            } else {
                                ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                    for (byte, level, _) in &headings {
                                        let line_end = self.content[*byte..].find('\n').map_or(self.content.len(), |i| byte + i);
                                        let title = self.content[*byte..line_end].trim_start_matches('#').trim();
                                        let indent = (*level as f32 - 1.0).max(0.0) * 10.0;
                                        ui.horizontal(|ui| {
                                            ui.add_space(indent);
                                            if ui.selectable_label(false, RichText::new(title).size(12.5).color(colors::TEXT_PRIMARY)).clicked() {
                                                self.pending_select = Some((*byte, *byte));
                                                self.outline_open = false;
                                            }
                                        });
                                    }
                                });
                            }
                        });
                });
        }

        if self.find_open {
            egui::Area::new(egui::Id::new("find_bar"))
                .anchor(Align2::RIGHT_TOP, Vec2::new(-16.0, TITLE_STRIP + 2.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(colors::BG_CARD)
                        .stroke(Stroke::new(1.0_f32, colors::BORDER_LIGHT))
                        .corner_radius(8.0)
                        .shadow(ctx.style().visuals.popup_shadow)
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                                paint_icon(ui.painter(), r, Icon::Search, colors::TEXT_MUTED);
                                ui.add_space(4.0);
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut self.find_query)
                                        .frame(false)
                                        .hint_text(RichText::new("Find in note").color(colors::TEXT_MUTED))
                                        .desired_width(200.0),
                                );
                                if std::mem::take(&mut self.find_focus) {
                                    resp.request_focus();
                                }
                                if resp.changed() {
                                    self.find_match_index = 0;
                                    self.update_find_matches();
                                    self.select_current_match();
                                }
                                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    let back = ui.input(|i| i.modifiers.shift);
                                    self.step_match(!back);
                                    resp.request_focus();
                                }

                                let count = if self.find_matches.is_empty() {
                                    if self.find_query.is_empty() { String::new() } else { "0".to_string() }
                                } else {
                                    format!("{}/{}", self.find_match_index + 1, self.find_matches.len())
                                };
                                ui.add_sized(Vec2::new(40.0, 22.0), egui::Label::new(RichText::new(count).size(11.5).color(colors::TEXT_MUTED)));
                                if icon_button_sized(ui, Icon::Up, 22.0).on_hover_text("Previous  ⇧↩").clicked() {
                                    self.step_match(false);
                                }
                                if icon_button_sized(ui, Icon::Down, 22.0).on_hover_text("Next  ↩").clicked() {
                                    self.step_match(true);
                                }
                                if icon_button_sized(ui, Icon::Close, 22.0).on_hover_text("Close  Esc").clicked() {
                                    self.find_open = false;
                                }
                            });
                        });
                });
        }
    }

    fn error_toast(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = &self.error else { return };
        if at.elapsed() > Duration::from_secs(5) {
            self.error = None;
            return;
        }
        let msg = msg.clone();
        ctx.request_repaint_after(Duration::from_millis(500));
        egui::Area::new(egui::Id::new("error_toast"))
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(48, 24, 26))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(90, 40, 44)))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(msg).size(12.5).color(colors::TEXT_PRIMARY));
                    });
            });
    }

    /// Smart list, task, and blockquote continuation when pressing Enter.
    fn smart_enter_continuation(&mut self, ctx: &egui::Context, id: egui::Id, output: &egui::text_edit::TextEditOutput, prev_len: usize) {
        if self.content.len() != prev_len + 1 {
            return;
        }
        let Some(range) = output.cursor_range else { return };
        if range.primary.index != range.secondary.index {
            return;
        }
        let cur = char_to_byte(&self.content, range.primary.index);
        if cur == 0 || self.content.as_bytes().get(cur - 1) != Some(&b'\n') {
            return;
        }
        let prev_end = cur - 1;
        let prev_start = self.content[..prev_end].rfind('\n').map_or(0, |i| i + 1);
        let prev_line = &self.content[prev_start..prev_end];
        let indent_len = prev_line.len() - prev_line.trim_start().len();
        let (indent, rest) = prev_line.split_at(indent_len);

        let next_prefix: Option<(String, bool)> = if let Some(after) = rest
            .strip_prefix("- [ ] ")
            .or_else(|| rest.strip_prefix("- [x] "))
            .or_else(|| rest.strip_prefix("- [X] "))
        {
            Some((format!("{indent}- [ ] "), after.trim().is_empty()))
        } else if let Some(b) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")).or_else(|| rest.strip_prefix("+ ")) {
            let marker = &rest[..2];
            Some((format!("{indent}{marker}"), b.trim().is_empty()))
        } else if let Some(after) = rest.strip_prefix("> ") {
            Some((format!("{indent}> "), after.trim().is_empty()))
        } else {
            let digits = rest.bytes().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && rest.len() > digits + 1 && matches!(rest.as_bytes()[digits], b'.' | b')') && rest.as_bytes()[digits + 1] == b' ' {
                let num: usize = rest[..digits].parse().unwrap_or(1);
                let sep = rest.as_bytes()[digits] as char;
                let after = &rest[digits + 2..];
                Some((format!("{indent}{}{sep} ", num + 1), after.trim().is_empty()))
            } else {
                None
            }
        };

        let Some((prefix, was_empty)) = next_prefix else { return };
        let mut state = output.state.clone();
        if was_empty {
            self.content.replace_range(prev_start..cur, "");
            self.mark_edited();
            let c = egui::text::CCursor::new(self.content[..prev_start].chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(c)));
        } else {
            self.content.insert_str(cur, &prefix);
            self.mark_edited();
            let c = egui::text::CCursor::new(self.content[..cur + prefix.len()].chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(c)));
        }
        state.store(ctx, id);
        ctx.request_repaint();
    }

    /// Floating `[[wiki-link]]` autocomplete popup anchored near the cursor.
    fn wiki_autocomplete(&mut self, ctx: &egui::Context, id: egui::Id, output: &egui::text_edit::TextEditOutput) {
        if !output.response.has_focus() {
            return;
        }
        let Some(range) = output.cursor_range else { return };
        if range.primary.index != range.secondary.index {
            return;
        }
        let cur = char_to_byte(&self.content, range.primary.index);
        let line_start = self.content[..cur].rfind('\n').map_or(0, |i| i + 1);
        let before = &self.content[line_start..cur];
        let Some(rel_open) = before.rfind("[[") else {
            self.ac_dismissed_at = None;
            return;
        };
        if before[rel_open + 2..].contains("]]") {
            self.ac_dismissed_at = None;
            return;
        }
        let open_byte = line_start + rel_open;
        if self.ac_dismissed_at == Some(open_byte) {
            return;
        }
        let query = before[rel_open + 2..].trim().to_lowercase();
        let matches: Vec<String> = self
            .store
            .files
            .iter()
            .filter(|f| query.is_empty() || f.search_key.contains(&query))
            .take(8)
            .map(|f| f.title().to_string())
            .collect();
        if matches.is_empty() {
            return;
        }
        if self.ac_index >= matches.len() {
            self.ac_index = 0;
        }

        let mut accept: Option<String> = None;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
            self.ac_index = (self.ac_index + 1) % matches.len();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
            self.ac_index = (self.ac_index + matches.len() - 1) % matches.len();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)) {
            accept = Some(matches[self.ac_index].clone());
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.ac_dismissed_at = Some(open_byte);
            return;
        }

        let cursor_rect = output.galley.pos_from_cursor(range.primary).translate(output.galley_pos.to_vec2());
        egui::Area::new(egui::Id::new("wiki_autocomplete"))
            .fixed_pos(Pos2::new(cursor_rect.left(), cursor_rect.bottom() + 4.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(colors::BG_CARD)
                    .stroke(Stroke::new(1.0_f32, colors::BORDER_LIGHT))
                    .corner_radius(8.0)
                    .shadow(ctx.style().visuals.popup_shadow)
                    .inner_margin(egui::Margin::same(4))
                    .show(ui, |ui| {
                        ui.set_width(220.0);
                        for (idx, title) in matches.iter().enumerate() {
                            let selected = idx == self.ac_index;
                            if ui.selectable_label(selected, RichText::new(title).size(12.5).color(colors::TEXT_PRIMARY)).clicked() {
                                accept = Some(title.clone());
                            }
                        }
                    });
            });

        if let Some(title) = accept {
            let replace_start = open_byte + 2;
            let already_closed = self.content[cur..].starts_with("]]");
            let inserted = if already_closed { title.clone() } else { format!("{title}]]") };
            self.content.replace_range(replace_start..cur, &inserted);
            self.mark_edited();
            let end_byte = replace_start + title.len() + 2;
            let mut state = output.state.clone();
            let c = egui::text::CCursor::new(self.content[..end_byte.min(self.content.len())].chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(c)));
            state.store(ctx, id);
            self.ac_dismissed_at = None;
            ctx.request_repaint();
        }
    }

    fn quick_switcher(&mut self, ctx: &egui::Context) {
        if !self.quick_open {
            return;
        }
        let q = self.quick_query.trim().to_lowercase();
        let mut results: Vec<(&MarkdownFile, u8)> = self
            .store
            .files
            .iter()
            .filter_map(|f| {
                if q.is_empty() {
                    return Some((f, 0));
                }
                if f.title().to_lowercase().contains(&q) {
                    Some((f, 0))
                } else if f.search_key.contains(&q) {
                    Some((f, 1))
                } else if self.store.meta_index.get(&f.relative_path).is_some_and(|m| {
                    let tq = q.strip_prefix('#').unwrap_or(&q);
                    m.tags.iter().any(|t| t.contains(tq)) || m.content_lower.contains(&q)
                }) {
                    Some((f, 2))
                } else {
                    None
                }
            })
            .collect();
        results.sort_by_key(|(_, rank)| *rank);
        results.truncate(10);
        let len = results.len();
        if self.quick_index >= len {
            self.quick_index = 0;
        }

        if len > 0 && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
            self.quick_index = (self.quick_index + 1) % len;
        }
        if len > 0 && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
            self.quick_index = (self.quick_index + len - 1) % len;
        }

        let mut open_path: Option<String> = None;
        let mut create_name: Option<String> = None;

        egui::Area::new(egui::Id::new("quick_scrim"))
            .fixed_pos(Pos2::ZERO)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                let screen = ui.ctx().content_rect();
                if ui.allocate_rect(screen, Sense::click()).clicked() {
                    self.quick_open = false;
                }
                ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(100));
            });

        egui::Window::new("quick_switcher")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 80.0))
            .order(egui::Order::Foreground)
            .frame(
                egui::Frame::new()
                    .fill(colors::BG_CARD)
                    .corner_radius(12.0)
                    .stroke(Stroke::new(1.0_f32, colors::BORDER_LIGHT))
                    .shadow(ctx.style().visuals.window_shadow)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.set_width(420.0);
                let resp = text_field(ui, &mut self.quick_query, "Jump to note, #tag, or text… (⇧↩ create)");
                if std::mem::take(&mut self.quick_focus) {
                    resp.request_focus();
                }
                if resp.changed() {
                    self.quick_index = 0;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let shift = ui.input(|i| i.modifiers.shift);
                    if shift && !self.quick_query.trim().is_empty() {
                        create_name = Some(self.quick_query.trim().to_string());
                    } else if let Some((f, _)) = results.get(self.quick_index) {
                        open_path = Some(f.relative_path.clone());
                    }
                }
                ui.add_space(8.0);
                if results.is_empty() {
                    ui.label(RichText::new("No matching notes — press ⇧↩ to create").size(12.0).color(colors::TEXT_MUTED));
                } else {
                    for (idx, (file, rank)) in results.iter().enumerate() {
                        let sel = idx == self.quick_index;
                        let r = file_row(ui, file, sel, *rank == 2);
                        if r.clicked() {
                            open_path = Some(file.relative_path.clone());
                        }
                    }
                }
            });

        if let Some(name) = create_name {
            let content = format!("# {}\n\n", name.rsplit('/').next().unwrap_or(&name));
            match self.store.create_file(&name, &content) {
                Ok(path) => {
                    self.quick_open = false;
                    self.load_note(&path);
                }
                Err(e) => self.show_error(format!("Couldn't create note: {e}")),
            }
        } else if let Some(path) = open_path {
            self.quick_open = false;
            self.load_note(&path);
        }
    }

    // -----------------------------------------------------------------------
    // Dialogs
    // -----------------------------------------------------------------------

    fn render_dialogs(&mut self, ctx: &egui::Context) {
        if self.show_create_dialog {
            let mut create = false;
            let mut cancel = false;
            dialog(ctx, "create_dialog", |ui| {
                ui.label(RichText::new("New note").size(14.0).color(colors::TEXT_PRIMARY));
                ui.add_space(10.0);
                let resp = text_field(ui, &mut self.new_note_input, "Name, or folder/name");
                create |= ui.input(|i| i.key_pressed(egui::Key::Enter));
                resp.request_focus();
                ui.add_space(14.0);
                let (ok, no) = dialog_buttons(ui, "Create", false);
                create |= ok;
                cancel = no;
            });
            if cancel {
                self.show_create_dialog = false;
            }
            if create {
                let name = self.new_note_input.trim().to_string();
                if !name.is_empty() {
                    let default_content = format!("# {}\n\n", name.rsplit('/').next().unwrap_or(&name));
                    match self.store.create_file(&name, &default_content) {
                        Ok(path) => {
                            self.load_note(&path);
                            self.show_create_dialog = false;
                            self.is_source_mode = false;
                        }
                        Err(e) => self.show_error(format!("Couldn't create note: {e}")),
                    }
                }
            }
        }

        if self.show_rename_dialog {
            let mut rename = false;
            let mut cancel = false;
            dialog(ctx, "rename_dialog", |ui| {
                ui.label(RichText::new("Rename note").size(14.0).color(colors::TEXT_PRIMARY));
                ui.add_space(10.0);
                let resp = text_field(ui, &mut self.rename_input, "New name");
                rename |= ui.input(|i| i.key_pressed(egui::Key::Enter));
                resp.request_focus();
                ui.add_space(14.0);
                let (ok, no) = dialog_buttons(ui, "Rename", false);
                rename |= ok;
                cancel = no;
            });
            if cancel {
                self.show_rename_dialog = false;
            }
            if rename {
                let new_name = self.rename_input.trim().to_string();
                if let (false, Some(old_path)) = (new_name.is_empty(), self.selected_path.clone()) {
                    if self.is_dirty {
                        self.save_current();
                    }
                    match self.store.rename_file(&old_path, &new_name) {
                        Ok(new_path) => {
                            self.show_rename_dialog = false;
                            self.load_note(&new_path);
                        }
                        Err(e) => self.show_error(format!("Couldn't rename: {e}")),
                    }
                }
            }
        }

        if self.show_delete_dialog {
            let mut delete = false;
            let mut cancel = false;
            let name = self.selected_path.clone().unwrap_or_default();
            let name = name.strip_suffix(".md").unwrap_or(&name).to_string();
            dialog(ctx, "delete_dialog", |ui| {
                ui.label(RichText::new(format!("Delete “{name}”?")).size(14.0).color(colors::TEXT_PRIMARY));
                ui.add_space(4.0);
                ui.label(RichText::new("The file is removed from disk.").size(12.5).color(colors::TEXT_MUTED));
                ui.add_space(14.0);
                let (ok, no) = dialog_buttons(ui, "Delete", true);
                delete = ok || ui.input(|i| i.key_pressed(egui::Key::Enter));
                cancel = no;
            });
            if cancel {
                self.show_delete_dialog = false;
            }
            if delete {
                if let Some(path) = self.selected_path.clone() {
                    match self.store.delete_file(&path) {
                        Ok(_) => {
                            self.selected_path = None;
                            self.content.clear();
                            self.is_dirty = false;
                            self.show_delete_dialog = false;
                            if let Some(first) = self.store.files.first() {
                                let next = first.relative_path.clone();
                                self.load_note(&next);
                            }
                        }
                        Err(e) => self.show_error(format!("Couldn't delete: {e}")),
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Small widgets
// ---------------------------------------------------------------------------

/// One sidebar row: note title, with the folder as a faint suffix when nested.
fn file_row(ui: &mut egui::Ui, file: &MarkdownFile, selected: bool, content_match: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
    let title = file.title();
    let bg = if selected {
        colors::ROW_SELECTED
    } else if resp.hovered() {
        colors::ROW_HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 6.0, bg);

    let mut job = LayoutJob { wrap: TextWrapping::truncate_at_width(rect.width() - 20.0), ..Default::default() };
    job.append(
        title,
        0.0,
        TextFormat { font_id: FontId::proportional(13.0), color: if selected { Color32::WHITE } else { Color32::from_rgb(178, 180, 190) }, ..Default::default() },
    );
    if let Some((dir, _)) = file.relative_path.rsplit_once('/') {
        job.append(dir, 8.0, TextFormat { font_id: FontId::proportional(11.0), color: colors::TEXT_MUTED, ..Default::default() });
    }
    if content_match {
        job.append("match", 8.0, TextFormat { font_id: FontId::proportional(10.5), color: colors::TEXT_ACCENT, ..Default::default() });
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let pos = Pos2::new(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, colors::TEXT_SECONDARY);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Small rounded button used for the floating controls.
fn pill(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let fill = if active { colors::ACCENT_MUTED } else { Color32::from_white_alpha(7) };
    let color = if active { colors::TEXT_ACCENT } else { colors::TEXT_SECONDARY };
    ui.add(
        egui::Button::new(RichText::new(label).size(12.0).color(color))
            .fill(fill)
            .corner_radius(12.0)
            .min_size(Vec2::new(64.0, 24.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn dialog(ctx: &egui::Context, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    // Dim the window behind the card.
    egui::Area::new(egui::Id::new((id, "scrim")))
        .fixed_pos(Pos2::ZERO)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let screen = ui.ctx().content_rect();
            ui.allocate_rect(screen, Sense::click());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(110));
        });
    egui::Window::new(id)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::new(0.0, -40.0))
        .order(egui::Order::Foreground)
        .frame(
            egui::Frame::new()
                .fill(colors::BG_CARD)
                .corner_radius(12.0)
                .stroke(Stroke::new(1.0_f32, colors::BORDER_LIGHT))
                .shadow(ctx.style().visuals.window_shadow)
                .inner_margin(egui::Margin::same(18)),
        )
        .show(ctx, |ui| {
            ui.set_width(300.0);
            ui.set_max_width(300.0);
            add(ui);
        });
}

fn text_field(ui: &mut egui::Ui, value: &mut String, hint: &str) -> egui::Response {
    egui::Frame::new()
        .fill(colors::BG_INPUT)
        .corner_radius(7.0)
        .stroke(Stroke::new(1.0_f32, colors::BORDER_LIGHT))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(value)
                    .frame(false)
                    .hint_text(RichText::new(hint).color(colors::TEXT_MUTED))
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(13.5)),
            )
        })
        .inner
}

/// Cancel + primary button, right-aligned. Returns (confirmed, cancelled).
fn dialog_buttons(ui: &mut egui::Ui, primary: &str, danger: bool) -> (bool, bool) {
    let mut ok = false;
    let mut cancel = false;
    // Fixed-height row: a bare right-to-left layout would claim all remaining height.
    let size = Vec2::new(ui.available_width(), 28.0);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        let fill = if danger { Color32::from_rgb(170, 52, 52) } else { colors::ACCENT };
        ok = ui
            .add(egui::Button::new(RichText::new(primary).size(12.5).color(Color32::WHITE)).fill(fill).corner_radius(7.0).min_size(Vec2::new(76.0, 28.0)))
            .clicked();
        cancel = ui
            .add(egui::Button::new(RichText::new("Cancel").size(12.5).color(colors::TEXT_SECONDARY)).corner_radius(7.0).min_size(Vec2::new(70.0, 28.0)))
            .clicked();
    });
    (ok, cancel)
}

fn open_url(ctx: &egui::Context, url: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = ctx;
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(not(target_os = "macos"))]
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

/// Converts sorted, non-overlapping byte ranges into char ranges in one pass over `text`.
fn byte_ranges_to_chars(text: &str, ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(ranges.len());
    let (mut byte, mut chars) = (0usize, 0usize);
    let mut to_char = |target: usize| {
        let target = target.min(text.len());
        chars += text[byte..target].chars().count();
        byte = target;
        chars
    };
    for &(a, b) in ranges {
        let ca = to_char(a);
        out.push((ca, to_char(b)));
    }
    out
}

fn char_range_to_bytes(text: &str, a: usize, b: usize) -> (usize, usize) {
    let (x, y) = (char_to_byte(text, a), char_to_byte(text, b));
    (x.min(y), x.max(y))
}

fn char_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map_or(text.len(), |(b, _)| b)
}

/// Turns on the macOS blurred-glass window background. Returns false where unsupported.
fn apply_glass(cc: &eframe::CreationContext<'_>) -> bool {
    #[cfg(target_os = "macos")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};

        if apply_vibrancy(cc, NSVisualEffectMaterial::HudWindow, Some(NSVisualEffectState::Active), None).is_err() {
            return false;
        }
        // window-vibrancy adds the blur as a *child* of egui's view, and children draw on top of
        // their parent, which would hide everything egui paints. Move it next to egui's view
        // instead, ordered below it, so egui draws over the blur.
        let Ok(handle) = cc.window_handle() else { return false };
        let RawWindowHandle::AppKit(h) = handle.as_raw() else { return false };
        // Tag window-vibrancy gives its blur view.
        const BLUR_VIEW_TAG: isize = 91376254;
        // SAFETY: called on the main thread during app creation; `ns_view` is the window's live
        // content view, owned by AppKit for the lifetime of the window.
        unsafe {
            let view: &objc2_app_kit::NSView = h.ns_view.cast().as_ref();
            let (Some(blur), Some(parent)) = (view.viewWithTag(BLUR_VIEW_TAG), view.superview()) else { return false };
            blur.removeFromSuperview();
            blur.setFrame(view.frame());
            parent.addSubview_positioned_relativeTo(&blur, objc2_app_kit::NSWindowOrderingMode::Below, Some(view));
        }
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cc;
        false
    }
}

// ---------------------------------------------------------------------------
// Icons (drawn with lines, so they look the same with any font)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Icon {
    Sidebar,
    Plus,
    Search,
    Close,
    Up,
    Down,
    Back,
    Forward,
    Outline,
}

fn paint_icon(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 2.0;
    let st = Stroke::new(1.4_f32, color);
    match icon {
        Icon::Sidebar => {
            let r = Rect::from_center_size(c, Vec2::new(s * 1.3, s * 1.05));
            p.rect_stroke(r, 2.5, st, egui::StrokeKind::Middle);
            let x = r.left() + r.width() * 0.36;
            p.line_segment([Pos2::new(x, r.top()), Pos2::new(x, r.bottom())], st);
        }
        Icon::Plus => {
            let d = s * 0.5;
            p.line_segment([c - Vec2::new(d, 0.0), c + Vec2::new(d, 0.0)], st);
            p.line_segment([c - Vec2::new(0.0, d), c + Vec2::new(0.0, d)], st);
        }
        Icon::Search => {
            let r = s * 0.42;
            let o = c - Vec2::splat(s * 0.12);
            p.circle_stroke(o, r, st);
            let d = Vec2::splat(std::f32::consts::FRAC_1_SQRT_2);
            p.line_segment([o + d * r, o + d * (r + s * 0.4)], st);
        }
        Icon::Close => {
            let d = s * 0.32;
            p.line_segment([c - Vec2::splat(d), c + Vec2::splat(d)], st);
            p.line_segment([c + Vec2::new(-d, d), c + Vec2::new(d, -d)], st);
        }
        Icon::Up | Icon::Down => {
            let d = s * 0.34;
            let dir = if matches!(icon, Icon::Up) { -1.0 } else { 1.0 };
            let tip = c + Vec2::new(0.0, dir * d * 0.5);
            p.line_segment([tip, tip + Vec2::new(-d, -dir * d)], st);
            p.line_segment([tip, tip + Vec2::new(d, -dir * d)], st);
        }
        Icon::Back | Icon::Forward => {
            let d = s * 0.34;
            let dir = if matches!(icon, Icon::Back) { -1.0 } else { 1.0 };
            let tip = c + Vec2::new(dir * d * 0.5, 0.0);
            p.line_segment([tip, tip + Vec2::new(-dir * d, -d)], st);
            p.line_segment([tip, tip + Vec2::new(-dir * d, d)], st);
        }
        Icon::Outline => {
            let w = s * 0.6;
            for (dy, inset) in [(-s * 0.36, 0.0), (0.0, s * 0.25), (s * 0.36, s * 0.25)] {
                p.line_segment([Pos2::new(c.x - w + inset, c.y + dy), Pos2::new(c.x + w, c.y + dy)], st);
            }
        }
    }
}

fn icon_button_sized(ui: &mut egui::Ui, icon: Icon, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, Color32::from_white_alpha(14));
    }
    let color = if resp.hovered() { colors::TEXT_PRIMARY } else { colors::TEXT_SECONDARY };
    paint_icon(ui.painter(), rect, icon, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn icon_button(ui: &mut egui::Ui, icon: Icon) -> egui::Response {
    icon_button_sized(ui, icon, 24.0)
}

fn slugify_anchor(s: &str) -> String {
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

/// Case-insensitive substring search that maps lowercase byte positions back to valid UTF-8 byte ranges in `haystack`.
fn find_case_insensitive(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle_lower = needle.to_lowercase();
    if needle_lower.is_empty() {
        return Vec::new();
    }
    let mut lower = String::with_capacity(haystack.len());
    let mut map: Vec<(usize, usize)> = Vec::with_capacity(haystack.len());
    for (orig_start, ch) in haystack.char_indices() {
        let orig_end = orig_start + ch.len_utf8();
        for lc in ch.to_lowercase() {
            let mut buf = [0u8; 4];
            let s = lc.encode_utf8(&mut buf);
            lower.push_str(s);
            for _ in 0..s.len() {
                map.push((orig_start, orig_end));
            }
        }
    }
    let mut out = Vec::new();
    let mut pos = 0;
    let nlen = needle_lower.len();
    while let Some(rel) = lower[pos..].find(&needle_lower) {
        let start = pos + rel;
        let end = start + nlen;
        if let (Some(&(a, _)), Some(&(_, b))) = (map.get(start), map.get(end - 1)) {
            out.push((a, b));
        }
        pos = end.max(start + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn byte_ranges_map_to_char_ranges() {
        let text = "aé b é";
        // "é" is two bytes: matches at bytes 1..3 and 6..8 are chars 1..2 and 5..6.
        assert_eq!(super::byte_ranges_to_chars(text, &[(1, 3), (6, 8)]), vec![(1, 2), (5, 6)]);
    }

    #[test]
    fn unicode_case_insensitive_find_never_panics() {
        let text = "İstanbul ẞ Straße";
        let hits = super::find_case_insensitive(text, "straße");
        assert_eq!(hits.len(), 1);
        assert_eq!(&text[hits[0].0..hits[0].1], "Straße");

        let hits2 = super::find_case_insensitive(text, "i\u{307}stanbul");
        assert_eq!(hits2.len(), 1);
        assert_eq!(&text[hits2[0].0..hits2[0].1], "İstanbul");
    }
}
