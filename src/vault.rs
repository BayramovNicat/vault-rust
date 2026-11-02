use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::SystemTime;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::wiki_links::{parse_wiki_links_with_lines, LinkIndex, WikiResolver};

/// Folders that never hold notes (besides hidden ones).
const SKIPPED_DIRS: [&str; 4] = ["node_modules", "dist", "build", "target"];

#[derive(Clone, Debug, PartialEq)]
pub struct MarkdownFile {
    pub relative_path: String,
    pub name: String,
    pub mtime: SystemTime,
    /// Lowercase `relative_path`, for the sidebar filter.
    pub search_key: String,
}

impl MarkdownFile {
    pub fn new(relative_path: String, mtime: SystemTime) -> Self {
        let name = relative_path.rsplit('/').next().unwrap_or(&relative_path).to_string();
        let search_key = relative_path.to_lowercase();
        Self { relative_path, name, mtime, search_key }
    }

    /// File name without the `.md` extension.
    pub fn title(&self) -> &str {
        self.name.strip_suffix(".md").unwrap_or(&self.name)
    }
}

pub struct VaultStore {
    pub root: PathBuf,
    /// Newest first.
    pub files: Vec<MarkdownFile>,
    pub link_index: LinkIndex,
    /// mtime of each file when its links were indexed, so external edits get re-indexed.
    indexed_mtime: HashMap<String, SystemTime>,
    resolver: WikiResolver,
    _watcher: Option<RecommendedWatcher>,
    watcher_rx: Receiver<notify::Result<Event>>,
}

impl VaultStore {
    /// Opens the vault (`VAULT_DIR` or `~/.vault`). `on_change` runs on the watcher thread
    /// whenever files change on disk, e.g. to wake up the UI.
    pub fn new(on_change: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let root = match std::env::var("VAULT_DIR") {
            Ok(dir) => PathBuf::from(dir),
            Err(_) => dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".vault"),
        };
        Self::open(root, on_change)
    }

    /// Opens the vault at `root`, creating the folder if needed.
    pub fn open(root: PathBuf, on_change: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(&root).unwrap_or(root);

        let (tx, watcher_rx) = channel();
        let watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                // Reads don't change anything.
                if matches!(&res, Ok(e) if matches!(e.kind, EventKind::Access(_))) {
                    return;
                }
                if tx.send(res).is_ok() {
                    on_change();
                }
            },
            notify::Config::default(),
        )
        .ok()
        .and_then(|mut w| w.watch(&root, RecursiveMode::Recursive).ok().map(|_| w));

        let mut store = Self {
            root,
            files: Vec::new(),
            link_index: HashMap::new(),
            indexed_mtime: HashMap::new(),
            resolver: WikiResolver::new(&[]),
            _watcher: watcher,
            watcher_rx,
        };
        store.refresh_files();
        Ok(store)
    }

    /// Resolves `[[links]]` against the current file list.
    pub fn resolver(&self) -> &WikiResolver {
        &self.resolver
    }

    /// Drains pending watcher events. Returns true if anything changed on disk.
    pub fn take_disk_changes(&self) -> bool {
        let mut changed = false;
        while let Ok(res) = self.watcher_rx.try_recv() {
            changed |= res.is_ok();
        }
        changed
    }

    pub fn file(&self, relative_path: &str) -> Option<&MarkdownFile> {
        self.files.iter().find(|f| f.relative_path == relative_path)
    }

    /// Rescans the vault folder. Only files that are new or changed since they were indexed get re-read.
    pub fn refresh_files(&mut self) {
        let mut list = Vec::new();
        Self::scan_dir(&self.root, &self.root, &mut list);
        self.files = list;
        self.sort_files();

        let current: HashSet<&str> = self.files.iter().map(|f| f.relative_path.as_str()).collect();
        self.link_index.retain(|k, _| current.contains(k.as_str()));
        self.indexed_mtime.retain(|k, _| current.contains(k.as_str()));

        let stale: Vec<(String, SystemTime)> = self
            .files
            .iter()
            .filter(|f| self.indexed_mtime.get(&f.relative_path) != Some(&f.mtime))
            .map(|f| (f.relative_path.clone(), f.mtime))
            .collect();
        for (path, mtime) in stale {
            if let Ok(content) = self.read_file(&path) {
                self.index(&path, &content, mtime);
            }
        }
        self.rebuild_resolver();
    }

    fn scan_dir(root: &Path, current: &Path, results: &mut Vec<MarkdownFile>) {
        let Ok(entries) = fs::read_dir(current) else { return };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_ref()) {
                continue;
            }
            // `file_type` comes from the directory listing: no extra stat call.
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            // Symlinks are followed, like before.
            let is_dir = if kind.is_symlink() { path.is_dir() } else { kind.is_dir() };
            if is_dir {
                Self::scan_dir(root, &path, results);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
                let meta = if kind.is_symlink() { fs::metadata(&path) } else { entry.metadata() };
                let Ok(meta) = meta else { continue };
                if !meta.is_file() {
                    continue;
                }
                if let Ok(rel) = path.strip_prefix(root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    results.push(MarkdownFile::new(rel, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
                }
            }
        }
    }

    fn sort_files(&mut self) {
        self.files.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.relative_path.cmp(&b.relative_path)));
    }

    fn rebuild_resolver(&mut self) {
        self.resolver = WikiResolver::new(&self.files);
    }

    fn index(&mut self, relative_path: &str, content: &str, mtime: SystemTime) {
        self.link_index.insert(relative_path.to_string(), parse_wiki_links_with_lines(content));
        self.indexed_mtime.insert(relative_path.to_string(), mtime);
    }

    fn mtime_of(&self, relative_path: &str) -> SystemTime {
        fs::metadata(self.resolve_path(relative_path)).and_then(|m| m.modified()).unwrap_or_else(|_| SystemTime::now())
    }

    /// Records that `relative_path` was just written by us: no folder rescan needed.
    fn note_written(&mut self, relative_path: &str, content: &str) {
        let mtime = self.mtime_of(relative_path);
        self.index(relative_path, content, mtime);
        match self.files.iter_mut().find(|f| f.relative_path == relative_path) {
            Some(f) => f.mtime = mtime,
            None => {
                self.files.push(MarkdownFile::new(relative_path.to_string(), mtime));
                self.rebuild_resolver();
            }
        }
        self.sort_files();
    }

    pub fn read_file(&self, relative_path: &str) -> std::io::Result<String> {
        fs::read_to_string(self.resolve_path(relative_path))
    }

    pub fn write_file(&mut self, relative_path: &str, content: &str) -> std::io::Result<()> {
        let full_path = self.resolve_path(relative_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(full_path, content)?;
        self.note_written(relative_path, content);
        Ok(())
    }

    pub fn create_file(&mut self, relative_path: &str, content: &str) -> std::io::Result<String> {
        let normalized = Self::normalize_filename(relative_path);
        if self.resolve_path(&normalized).exists() {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "File already exists"));
        }
        self.write_file(&normalized, content)?;
        Ok(normalized)
    }

    pub fn rename_file(&mut self, old_path: &str, new_name: &str) -> std::io::Result<String> {
        let normalized_new = Self::normalize_filename(new_name);
        // Keep the folder unless the new name gives one.
        let new_rel = match old_path.rsplit_once('/') {
            Some((dir, _)) if !normalized_new.contains('/') => format!("{dir}/{normalized_new}"),
            _ => normalized_new,
        };

        let new_full = self.resolve_path(&new_rel);
        if new_full.exists() {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "Target file already exists"));
        }
        if let Some(parent) = new_full.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(self.resolve_path(old_path), &new_full)?;

        if let Some(links) = self.link_index.remove(old_path) {
            self.link_index.insert(new_rel.clone(), links);
        }
        if let Some(mtime) = self.indexed_mtime.remove(old_path) {
            self.indexed_mtime.insert(new_rel.clone(), mtime);
        }
        if let Some(f) = self.files.iter_mut().find(|f| f.relative_path == old_path) {
            *f = MarkdownFile::new(new_rel.clone(), f.mtime);
        }
        self.sort_files();
        self.rebuild_resolver();
        Ok(new_rel)
    }

    pub fn delete_file(&mut self, relative_path: &str) -> std::io::Result<()> {
        let full_path = self.resolve_path(relative_path);
        if full_path.exists() {
            fs::remove_file(full_path)?;
        }
        self.link_index.remove(relative_path);
        self.indexed_mtime.remove(relative_path);
        self.files.retain(|f| f.relative_path != relative_path);
        self.rebuild_resolver();
        Ok(())
    }

    pub fn resolve_path(&self, relative_path: &str) -> PathBuf {
        self.root.join(relative_path.trim_start_matches('/'))
    }

    pub fn normalize_filename(name: &str) -> String {
        let trimmed = name.trim();
        let has_ext = trimmed.len().checked_sub(3).and_then(|i| trimmed.get(i..)).is_some_and(|e| e.eq_ignore_ascii_case(".md"));
        if has_ext { trimmed.to_string() } else { format!("{trimmed}.md") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault(name: &str) -> (PathBuf, VaultStore) {
        let dir = std::env::temp_dir().join(format!("vault-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = VaultStore::open(dir.clone(), || {}).unwrap();
        (dir, store)
    }

    #[test]
    fn edits_update_the_file_list_without_a_rescan() {
        let (dir, mut store) = temp_vault("edits");
        store.create_file("a", "links to [[b]]").unwrap();
        store.create_file("sub/b", "# B").unwrap();
        assert_eq!(store.files.len(), 2);
        assert_eq!(store.file("sub/b.md").unwrap().name, "b.md");
        assert_eq!(store.resolver().resolve("b").as_deref(), Some("sub/b.md"));
        assert_eq!(store.resolver().find_backlinks("sub/b.md", &store.link_index).len(), 1);

        let renamed = store.rename_file("sub/b.md", "c").unwrap();
        assert_eq!(renamed, "sub/c.md");
        assert!(store.file("sub/b.md").is_none() && store.file("sub/c.md").is_some());
        assert_eq!(store.resolver().resolve("c").as_deref(), Some("sub/c.md"));

        store.delete_file("a.md").unwrap();
        assert_eq!(store.files.len(), 1);
        assert!(!store.link_index.contains_key("a.md"));

        // What we track matches a fresh scan.
        let tracked: Vec<String> = store.files.iter().map(|f| f.relative_path.clone()).collect();
        store.refresh_files();
        let scanned: Vec<String> = store.files.iter().map(|f| f.relative_path.clone()).collect();
        assert_eq!(tracked, scanned);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn external_edits_are_reindexed() {
        let (dir, mut store) = temp_vault("external");
        store.create_file("a", "no links").unwrap();
        assert!(store.link_index["a.md"].is_empty());
        // Another app rewrites the note (with a different mtime).
        let path = store.resolve_path("a.md");
        fs::write(&path, "now [[b]]").unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        store.refresh_files();
        assert_eq!(store.link_index["a.md"].len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn normalizes_names() {
        assert_eq!(VaultStore::normalize_filename(" note "), "note.md");
        assert_eq!(VaultStore::normalize_filename("Note.MD"), "Note.MD");
        assert_eq!(VaultStore::normalize_filename("é"), "é.md");
    }
}
