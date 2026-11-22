use std::collections::HashMap;
use crate::vault::MarkdownFile;

#[derive(Clone, Debug, PartialEq)]
pub struct WikiLink {
    pub target: String,
    pub heading: Option<String>,
    pub alias: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BacklinkMatch {
    pub line_number: usize,
    pub line_text: String,
}

#[derive(Clone, Debug)]
pub struct Backlink {
    pub source_path: String,
    pub matches: Vec<BacklinkMatch>,
}

pub type LinkIndex = HashMap<String, Vec<(usize, String, WikiLink)>>;

/// Strips inline backtick code spans from a single line so `[[...]]` inside `` `code` `` is ignored.
fn parse_wiki_links_in_line(line: &str) -> Vec<WikiLink> {
    let mut links = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;

    while i + 1 < bytes.len() {
        if bytes[i] == b'`' {
            let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            let delim = &line[i..i + ticks];
            if let Some(rel) = line[i + ticks..].find(delim) {
                i = i + ticks + rel + ticks;
                continue;
            }
            i += ticks;
            continue;
        }
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            if let Some(end_offset) = line[start..].find("]]") {
                let inner = line[start..start + end_offset].trim();
                if !inner.is_empty() && !inner.contains('\n') && !inner.contains('\r') {
                    let (target_part, alias) = match inner.split_once('|') {
                        Some((t, a)) => (t.trim(), Some(a.trim().to_string())),
                        None => (inner, None),
                    };
                    let (target, heading) = match target_part.split_once('#') {
                        Some((t, h)) => (t.trim().to_string(), Some(h.trim().to_string())),
                        None => (target_part.to_string(), None),
                    };
                    if !target.is_empty() {
                        links.push(WikiLink { target, heading, alias });
                    }
                }
                i = start + end_offset + 2;
                continue;
            }
        }
        i += 1;
    }

    links
}

#[cfg(test)]
pub fn parse_wiki_links(content: &str) -> Vec<WikiLink> {
    let mut links = Vec::new();
    for line in content.lines() {
        links.extend(parse_wiki_links_in_line(line));
    }
    links
}

pub fn parse_wiki_links_with_lines(content: &str) -> Vec<(usize, String, WikiLink)> {
    let mut results = Vec::new();
    let mut fence_char: Option<u8> = None;
    let mut fence_len = 0usize;

    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(fc) = fence_char {
            let run = trimmed.bytes().take_while(|&b| b == fc).count();
            if run >= fence_len && trimmed[run..].trim().is_empty() {
                fence_char = None;
            }
            continue;
        } else if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let fc = trimmed.as_bytes()[0];
            fence_len = trimmed.bytes().take_while(|&b| b == fc).count();
            fence_char = Some(fc);
            continue;
        }

        if line.contains("[[") {
            for link in parse_wiki_links_in_line(line) {
                results.push((idx + 1, line.trim().to_string(), link));
            }
        }
    }
    results
}

/// Rewrites `[[target]]` references in `content` that point to `old_path` so they point to `new_path`.
/// Returns `None` if no links changed.
pub fn rewrite_wiki_links(content: &str, old_path: &str, new_path: &str, resolver: &WikiResolver) -> Option<String> {
    let old_dir = old_path.rsplit_once('/').map(|(d, _)| d);
    let new_dir = new_path.rsplit_once('/').map(|(d, _)| d);
    let new_name = new_path.rsplit('/').next().unwrap_or(new_path);
    let new_name_no_ext = new_name.strip_suffix(".md").unwrap_or(new_name);
    let new_path_no_ext = new_path.strip_suffix(".md").unwrap_or(new_path);

    let mut out = String::with_capacity(content.len());
    let mut changed = false;
    let mut fence_char: Option<u8> = None;
    let mut fence_len = 0usize;

    for segment in content.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let nl = &segment[line.len()..];
        let trimmed = line.trim_start();

        if let Some(fc) = fence_char {
            let run = trimmed.bytes().take_while(|&b| b == fc).count();
            if run >= fence_len && trimmed[run..].trim().is_empty() {
                fence_char = None;
            }
            out.push_str(segment);
            continue;
        } else if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let fc = trimmed.as_bytes()[0];
            fence_len = trimmed.bytes().take_while(|&b| b == fc).count();
            fence_char = Some(fc);
            out.push_str(segment);
            continue;
        }

        if !line.contains("[[") {
            out.push_str(segment);
            continue;
        }

        let bytes = line.as_bytes();
        let mut i = 0;
        let mut last_copied = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'`' {
                let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                let delim = &line[i..i + ticks];
                if let Some(rel) = line[i + ticks..].find(delim) {
                    i = i + ticks + rel + ticks;
                    continue;
                }
                i += ticks;
                continue;
            }
            if bytes[i] == b'[' && bytes[i + 1] == b'[' {
                let start = i + 2;
                if let Some(end_offset) = line[start..].find("]]") {
                    let inner = &line[start..start + end_offset];
                    let (target_and_heading, alias_suffix) = match inner.split_once('|') {
                        Some((t, _)) => (t, &inner[t.len()..]),
                        None => (inner, ""),
                    };
                    let (raw_target, heading_suffix) = match target_and_heading.split_once('#') {
                        Some((t, _)) => (t, &target_and_heading[t.len()..]),
                        None => (target_and_heading, ""),
                    };
                    let target_trimmed = raw_target.trim();
                    if !target_trimmed.is_empty() && resolver.resolve_ref(target_trimmed) == Some(old_path) {
                        let keep_ext = target_trimmed.to_ascii_lowercase().ends_with(".md");
                        let replacement = if target_trimmed.contains('/') || old_dir != new_dir {
                            if keep_ext { new_path } else { new_path_no_ext }
                        } else if keep_ext {
                            new_name
                        } else {
                            new_name_no_ext
                        };
                        out.push_str(&line[last_copied..start]);
                        out.push_str(replacement);
                        out.push_str(heading_suffix);
                        out.push_str(alias_suffix);
                        out.push_str("]]");
                        last_copied = start + end_offset + 2;
                        changed = true;
                    }
                    i = start + end_offset + 2;
                    continue;
                }
            }
            i += 1;
        }
        out.push_str(&line[last_copied..]);
        out.push_str(nl);
    }

    changed.then_some(out)
}

pub struct WikiResolver {
    // Maps lowercase keys to canonical relative paths
    lookup: HashMap<String, String>,
}

impl WikiResolver {
    pub fn new(files: &[MarkdownFile]) -> Self {
        let mut lookup = HashMap::new();

        for file in files {
            let rel = &file.relative_path;
            let rel_no_ext = rel.strip_suffix(".md").unwrap_or(rel);

            // Exact relative path
            lookup.insert(rel.to_lowercase(), rel.clone());
            lookup.insert(rel_no_ext.to_lowercase(), rel.clone());

            // Just file name without folder
            let name = &file.name;
            let name_no_ext = name.strip_suffix(".md").unwrap_or(name);
            lookup.entry(name.to_lowercase()).or_insert_with(|| rel.clone());
            lookup.entry(name_no_ext.to_lowercase()).or_insert_with(|| rel.clone());
        }

        Self { lookup }
    }

    pub fn resolve(&self, target: &str) -> Option<String> {
        self.resolve_ref(target).map(str::to_string)
    }

    /// Like `resolve`, without allocating the result.
    pub fn resolve_ref(&self, target: &str) -> Option<&str> {
        let key = target.trim().replace('\\', "/").to_lowercase();
        self.lookup
            .get(&key)
            .or_else(|| key.strip_suffix(".md").and_then(|k| self.lookup.get(k)))
            .map(String::as_str)
    }

    pub fn find_backlinks(
        &self,
        target_path: &str,
        link_index: &LinkIndex,
    ) -> Vec<Backlink> {
        let mut backlinks = Vec::new();
        let target_no_ext = target_path.strip_suffix(".md").unwrap_or(target_path);
        let target_name = target_path.rsplit('/').next().unwrap_or(target_path);
        let target_name_no_ext = target_name.strip_suffix(".md").unwrap_or(target_name);

        for (source_path, entries) in link_index {
            if source_path == target_path {
                continue;
            }

            let mut matches = Vec::new();
            for (line_num, line_str, link) in entries {
                let link_target = link.target.trim();
                let is_match = link_target.eq_ignore_ascii_case(target_path)
                    || link_target.eq_ignore_ascii_case(target_no_ext)
                    || link_target.eq_ignore_ascii_case(target_name)
                    || link_target.eq_ignore_ascii_case(target_name_no_ext)
                    || self.resolve_ref(link_target) == Some(target_path);

                if is_match {
                    matches.push(BacklinkMatch {
                        line_number: *line_num,
                        line_text: line_str.clone(),
                    });
                }
            }

            if !matches.is_empty() {
                backlinks.push(Backlink {
                    source_path: source_path.clone(),
                    matches,
                });
            }
        }

        backlinks.sort_by(|a, b| a.source_path.cmp(&b.source_path));
        backlinks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_wiki_links() {
        let content = "Check [[Note 1#Section]] and [[Folder/Note 2|Alias]] for info.";
        let links = parse_wiki_links(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "Note 1");
        assert_eq!(links[0].heading.as_deref(), Some("Section"));
        assert_eq!(links[0].alias, None);
        assert_eq!(links[1].target, "Folder/Note 2");
        assert_eq!(links[1].heading, None);
        assert_eq!(links[1].alias, Some("Alias".to_string()));
    }

    #[test]
    fn test_ignores_code_blocks_and_inline_code() {
        let content = "Real [[Note A]] and `[[Fake Inline]]`.\n```md\n[[Fake Fenced]]\n```\nAfter [[Note B]].";
        let links = parse_wiki_links_with_lines(content);
        let targets: Vec<&str> = links.iter().map(|(_, _, l)| l.target.as_str()).collect();
        assert_eq!(targets, vec!["Note A", "Note B"]);
    }

    #[test]
    fn test_wiki_resolver_and_backlinks() {
        let files = vec![
            MarkdownFile::new("work/ideas.md".to_string(), std::time::SystemTime::UNIX_EPOCH),
            MarkdownFile::new("journal.md".to_string(), std::time::SystemTime::UNIX_EPOCH),
        ];

        let resolver = WikiResolver::new(&files);
        assert_eq!(resolver.resolve("journal").as_deref(), Some("journal.md"));
        assert_eq!(resolver.resolve("journal.md").as_deref(), Some("journal.md"));
        assert_eq!(resolver.resolve("ideas").as_deref(), Some("work/ideas.md"));
        assert_eq!(resolver.resolve("work/ideas").as_deref(), Some("work/ideas.md"));
        assert_eq!(resolver.resolve("nonexistent"), None);

        let mut link_index = LinkIndex::new();
        link_index.insert(
            "journal.md".to_string(),
            vec![(
                1,
                "Today I had some [[ideas#top]].".to_string(),
                WikiLink {
                    target: "ideas".to_string(),
                    heading: Some("top".to_string()),
                    alias: None,
                },
            )],
        );

        let backlinks = resolver.find_backlinks("work/ideas.md", &link_index);
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].source_path, "journal.md");
        assert_eq!(backlinks[0].matches[0].line_number, 1);

        let updated = rewrite_wiki_links(
            "See [[ideas#top|my ideas]] and `[[ideas]]`.",
            "work/ideas.md",
            "work/concepts.md",
            &resolver,
        )
        .unwrap();
        assert_eq!(updated, "See [[concepts#top|my ideas]] and `[[ideas]]`.");
    }
}
