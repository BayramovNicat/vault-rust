use std::collections::HashMap;
use crate::vault::MarkdownFile;

#[derive(Clone, Debug, PartialEq)]
pub struct WikiLink {
    pub target: String,
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

pub fn parse_wiki_links(content: &str) -> Vec<WikiLink> {
    let mut links = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;

    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            if let Some(end_offset) = content[start..].find("]]") {
                let inner = content[start..start + end_offset].trim();
                if !inner.is_empty() && !inner.contains('\n') && !inner.contains('\r') {
                    if let Some((target, alias)) = inner.split_once('|') {
                        links.push(WikiLink {
                            target: target.trim().to_string(),
                            alias: Some(alias.trim().to_string()),
                        });
                    } else {
                        links.push(WikiLink {
                            target: inner.to_string(),
                            alias: None,
                        });
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

pub fn parse_wiki_links_with_lines(content: &str) -> Vec<(usize, String, WikiLink)> {
    let mut results = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        if line.contains("[[") {
            let links = parse_wiki_links(line);
            for link in links {
                results.push((idx + 1, line.trim().to_string(), link));
            }
        }
    }
    results
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
        let content = "Check [[Note 1]] and [[Folder/Note 2|Alias]] for info.";
        let links = parse_wiki_links(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "Note 1");
        assert_eq!(links[0].alias, None);
        assert_eq!(links[1].target, "Folder/Note 2");
        assert_eq!(links[1].alias, Some("Alias".to_string()));
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
                "Today I had some [[ideas]].".to_string(),
                WikiLink {
                    target: "ideas".to_string(),
                    alias: None,
                },
            )],
        );

        let backlinks = resolver.find_backlinks("work/ideas.md", &link_index);
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].source_path, "journal.md");
        assert_eq!(backlinks[0].matches[0].line_number, 1);
    }
}
