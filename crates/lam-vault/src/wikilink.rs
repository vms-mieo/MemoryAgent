#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    pub target: String,
    pub label: Option<String>,
    pub anchor: Option<String>,
}

pub fn extract_wikilinks(text: &str) -> Vec<Wikilink> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut position = 0;
    while position + 4 <= bytes.len() {
        if bytes[position] == b'[' && bytes[position + 1] == b'[' {
            if let Some(end) = find_close(&bytes[position + 2..]) {
                let inner = &text[position + 2..position + 2 + end];
                if let Some(link) = parse_inner(inner) {
                    out.push(link);
                }
                position += 2 + end + 2;
                continue;
            }
        }
        position += 1;
    }
    out
}

fn find_close(bytes: &[u8]) -> Option<usize> {
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b']' && bytes[index + 1] == b']' {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn parse_inner(inner: &str) -> Option<Wikilink> {
    if inner.trim().is_empty() || inner.contains("[[") || inner.contains('\n') {
        return None;
    }
    let (target_part, label) = match inner.find('|') {
        Some(position) => {
            let label = inner[position + 1..].trim();
            (
                inner[..position].trim(),
                if label.is_empty() {
                    None
                } else {
                    Some(label.to_string())
                },
            )
        }
        None => (inner.trim(), None),
    };
    if target_part.is_empty() {
        return None;
    }
    let (target, anchor) = match target_part.find('#') {
        Some(position) => {
            let anchor = target_part[position + 1..].trim();
            (
                target_part[..position].trim().to_string(),
                if anchor.is_empty() {
                    None
                } else {
                    Some(anchor.to_string())
                },
            )
        }
        None => (target_part.to_string(), None),
    };
    if target.is_empty() {
        return None;
    }
    Some(Wikilink {
        target,
        label,
        anchor,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    Exact(String),
    Unique(String),
    Ambiguous(Vec<String>),
    Missing,
}

pub fn resolve_link(known_paths: &[String], target: &str) -> LinkTarget {
    let trimmed = target.trim().trim_start_matches(['.', '/']);
    if trimmed.is_empty() {
        return LinkTarget::Missing;
    }
    let candidates = [trimmed.to_string(), format!("{trimmed}.md")];
    for candidate in &candidates {
        if known_paths.iter().any(|path| path == candidate) {
            return LinkTarget::Exact(candidate.clone());
        }
    }
    let file_name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let stem = file_name.strip_suffix(".md").unwrap_or(file_name);
    let mut matches: Vec<String> = known_paths
        .iter()
        .filter(|path| {
            let base = path.rsplit('/').next().unwrap_or(path);
            let base_stem = base.strip_suffix(".md").unwrap_or(base);
            base == file_name || base_stem == stem
        })
        .cloned()
        .collect();
    matches.sort();
    matches.dedup();
    match matches.len() {
        0 => LinkTarget::Missing,
        1 => LinkTarget::Unique(matches.remove(0)),
        _ => LinkTarget::Ambiguous(matches),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_and_labeled_links() {
        let links = extract_wikilinks("See [[docs/adr.md]] and [[guide|User Guide]].");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "docs/adr.md");
        assert_eq!(links[0].label, None);
        assert_eq!(links[1].target, "guide");
        assert_eq!(links[1].label.as_deref(), Some("User Guide"));
    }

    #[test]
    fn extracts_anchors() {
        let links = extract_wikilinks("Jump to [[notes.md#section]].");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "notes.md");
        assert_eq!(links[0].anchor.as_deref(), Some("section"));
    }

    #[test]
    fn rejects_broken_markup() {
        assert!(extract_wikilinks("See [[unclosed here.").is_empty());
        assert!(extract_wikilinks("See [[ ]].").is_empty());
        assert!(extract_wikilinks("See [[a\nb]].").is_empty());
    }

    #[test]
    fn exact_path_wins() {
        let known = vec!["docs/adr.md".to_string(), "other/adr.md".to_string()];
        assert_eq!(
            resolve_link(&known, "docs/adr.md"),
            LinkTarget::Exact("docs/adr.md".to_string())
        );
    }

    #[test]
    fn extension_implied() {
        let known = vec!["docs/adr.md".to_string()];
        assert_eq!(
            resolve_link(&known, "docs/adr"),
            LinkTarget::Exact("docs/adr.md".to_string())
        );
    }

    #[test]
    fn unique_basename_resolves() {
        let known = vec!["docs/adr.md".to_string(), "other/guide.md".to_string()];
        assert_eq!(
            resolve_link(&known, "adr"),
            LinkTarget::Unique("docs/adr.md".to_string())
        );
    }

    #[test]
    fn ambiguous_basename_reported() {
        let known = vec!["a/notes.md".to_string(), "b/notes.md".to_string()];
        match resolve_link(&known, "notes") {
            LinkTarget::Ambiguous(options) => assert_eq!(options.len(), 2),
            other => panic!("expected ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn missing_target_reported() {
        let known = vec!["a/notes.md".to_string()];
        assert_eq!(resolve_link(&known, "ghost"), LinkTarget::Missing);
    }
}
