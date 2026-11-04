use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Markdown,
    Text,
    Json,
    Yaml,
    Toml,
    Csv,
}

impl FileKind {
    pub fn of(path: &Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|raw| raw.to_str())
            .map(|raw| raw.to_ascii_lowercase())
            .as_deref()
        {
            Some("md") | Some("markdown") => Some(FileKind::Markdown),
            Some("txt") => Some(FileKind::Text),
            Some("json") => Some(FileKind::Json),
            Some("yaml") | Some("yml") => Some(FileKind::Yaml),
            Some("toml") => Some(FileKind::Toml),
            Some("csv") => Some(FileKind::Csv),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedDoc {
    pub title: Option<String>,
    pub text: String,
    pub redacted: bool,
}

pub fn parse_bytes(
    kind: FileKind,
    bytes: &[u8],
    file_stem: &str,
) -> Result<ParsedDoc, lam_core::LamError> {
    let raw = String::from_utf8(bytes.to_vec()).map_err(|_| {
        lam_core::LamError::validation(
            "E_VAULT_ENCODING",
            "file is not valid UTF-8 text".to_string(),
        )
    })?;
    let text = raw.replace("\r\n", "\n");
    match kind {
        FileKind::Markdown => Ok(ParsedDoc {
            title: markdown_title(&text).or_else(|| Some(file_stem.to_string())),
            text,
            redacted: false,
        }),
        FileKind::Text => Ok(ParsedDoc {
            title: Some(file_stem.to_string()),
            text,
            redacted: false,
        }),
        FileKind::Json => Ok(parse_json(&text)),
        FileKind::Yaml | FileKind::Toml | FileKind::Csv => Ok(parse_lines(&text, file_stem)),
    }
}

fn markdown_title(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

fn parse_json(text: &str) -> ParsedDoc {
    let (cleaned, redacted) = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => {
            let (next, hits) = redact_json(value);
            (
                serde_json::to_string_pretty(&next).unwrap_or_else(|_| text.to_string()),
                hits > 0,
            )
        }
        Err(_) => {
            let (next, hits) = redact_lines(text);
            (next, hits > 0)
        }
    };
    ParsedDoc {
        title: None,
        text: cleaned,
        redacted,
    }
}

fn redact_json(value: serde_json::Value) -> (serde_json::Value, usize) {
    match value {
        serde_json::Value::String(text) => {
            if lam_core::secrets::find_secret_kind(&text).is_some() {
                (serde_json::Value::String("[REDACTED]".to_string()), 1)
            } else {
                (serde_json::Value::String(text), 0)
            }
        }
        serde_json::Value::Array(items) => {
            let mut hits = 0;
            let out = items
                .into_iter()
                .map(|item| {
                    let (next, count) = redact_json(item);
                    hits += count;
                    next
                })
                .collect();
            (serde_json::Value::Array(out), hits)
        }
        serde_json::Value::Object(map) => {
            let mut hits = 0;
            let mut out = serde_json::Map::with_capacity(map.len());
            for (key, item) in map {
                let (next, count) = redact_json(item);
                hits += count;
                out.insert(key, next);
            }
            (serde_json::Value::Object(out), hits)
        }
        other => (other, 0),
    }
}

fn parse_lines(text: &str, file_stem: &str) -> ParsedDoc {
    let (cleaned, hits) = redact_lines(text);
    ParsedDoc {
        title: Some(file_stem.to_string()),
        text: cleaned,
        redacted: hits > 0,
    }
}

fn redact_lines(text: &str) -> (String, usize) {
    let mut hits = 0;
    let mut out = Vec::new();
    for line in text.lines() {
        if lam_core::secrets::find_secret_kind(line).is_some() {
            hits += 1;
            out.push("[REDACTED]".to_string());
        } else {
            out.push(line.to_string());
        }
    }
    (out.join("\n"), hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_title_extracted() {
        let doc = parse_bytes(FileKind::Markdown, b"# Hello\n\nBody here", "file").expect("parse");
        assert_eq!(doc.title.as_deref(), Some("Hello"));
        assert!(!doc.redacted);
    }

    #[test]
    fn json_secrets_redacted() {
        let raw = br#"{"api_key": "ghp_fixture0000000000000000000000000000", "name": "svc"}"#;
        let doc = parse_bytes(FileKind::Json, raw, "config").expect("parse");
        assert!(doc.redacted);
        assert!(!doc.text.contains("ghp_fixture"));
        assert!(doc.text.contains("svc"));
    }

    #[test]
    fn malformed_json_falls_back_to_text() {
        let raw = b"{not json, password=hunter2 here}";
        let doc = parse_bytes(FileKind::Json, raw, "config").expect("parse");
        assert!(doc.redacted);
        assert!(!doc.text.contains("hunter2"));
    }

    #[test]
    fn yaml_lines_redacted() {
        let raw = b"token: ghp_fixture0000000000000000000000000000\nname: svc\n";
        let doc = parse_bytes(FileKind::Yaml, raw, "config").expect("parse");
        assert!(doc.redacted);
        assert!(!doc.text.contains("ghp_fixture"));
        assert!(doc.text.contains("svc"));
    }

    #[test]
    fn plain_prose_untouched() {
        let raw = b"Just some notes about Temporal.\n";
        let doc = parse_bytes(FileKind::Text, raw, "notes").expect("parse");
        assert!(!doc.redacted);
        assert!(doc.text.contains("Temporal"));
    }
}
