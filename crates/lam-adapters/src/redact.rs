use std::collections::HashMap;

pub struct Sanitized {
    pub value: HashMap<String, serde_json::Value>,
    pub redactions: usize,
}

const STRING_CAP: usize = 2000;
const PAYLOAD_CAP: usize = 32768;

fn denied_key(key: &str) -> bool {
    matches!(
        key,
        "transcript"
            | "full_transcript"
            | "transcript_path"
            | "raw_output"
            | "full_output"
            | "messages"
    )
}

pub fn sanitize_payload(payload: HashMap<String, serde_json::Value>) -> Sanitized {
    let mut redactions = 0;
    let mut value = HashMap::with_capacity(payload.len());
    for (key, item) in payload {
        if denied_key(&key) {
            redactions += 1;
            continue;
        }
        let (cleaned, hits) = sanitize_value(item);
        redactions += hits;
        value.insert(key, cleaned);
    }
    enforce_budget(&mut value);
    Sanitized { value, redactions }
}

fn sanitize_value(item: serde_json::Value) -> (serde_json::Value, usize) {
    match item {
        serde_json::Value::String(text) => {
            let (scrubbed, hits) = scrub_text(&text);
            (serde_json::Value::String(truncate(&scrubbed)), hits)
        }
        serde_json::Value::Array(items) => {
            let mut hits = 0;
            let mut out = Vec::with_capacity(items.len().min(64));
            for entry in items.into_iter().take(64) {
                let (cleaned, entry_hits) = sanitize_value(entry);
                hits += entry_hits;
                out.push(cleaned);
            }
            (serde_json::Value::Array(out), hits)
        }
        serde_json::Value::Object(map) => {
            let mut hits = 0;
            let mut out = serde_json::Map::with_capacity(map.len());
            for (key, entry) in map {
                if denied_key(&key) {
                    hits += 1;
                    continue;
                }
                let (cleaned, entry_hits) = sanitize_value(entry);
                hits += entry_hits;
                out.insert(key, cleaned);
            }
            (serde_json::Value::Object(out), hits)
        }
        other => (other, 0),
    }
}

fn truncate(text: &str) -> String {
    if text.chars().count() > STRING_CAP {
        text.chars().take(STRING_CAP).collect()
    } else {
        text.to_string()
    }
}

fn enforce_budget(value: &mut HashMap<String, serde_json::Value>) {
    let mut serialized = serde_json::to_string(value).map(|s| s.len()).unwrap_or(0);
    while serialized > PAYLOAD_CAP {
        let target = value
            .iter()
            .filter_map(|(key, item)| match item {
                serde_json::Value::String(text) if text.len() > 500 => {
                    Some((key.clone(), text.len()))
                }
                _ => None,
            })
            .max_by_key(|(_, len)| *len)
            .map(|(key, _)| key);
        match target {
            Some(key) => {
                if let Some(serde_json::Value::String(text)) = value.get_mut(&key) {
                    *text = text.chars().take(500).collect();
                }
                serialized = serde_json::to_string(value).map(|s| s.len()).unwrap_or(0);
            }
            None => break,
        }
    }
}

pub fn scrub_text(text: &str) -> (String, usize) {
    let mut hits = 0;
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.contains("PRIVATE KEY") && line.contains("-----") {
            hits += 1;
            lines.push("[REDACTED:private-key]".to_string());
            continue;
        }
        let (scrubbed, line_hits) = scrub_line(line);
        hits += line_hits;
        lines.push(scrubbed);
    }
    let joined = lines.join("\n");
    if text.ends_with('\n') && !joined.ends_with('\n') {
        (format!("{joined}\n"), hits)
    } else {
        (joined, hits)
    }
}

const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("oc_sk_", 8),
    ("ghp_", 4),
    ("gho_", 4),
    ("github_pat_", 4),
    ("sk-ant-", 4),
    ("sk-proj-", 4),
    ("sk_live_", 16),
    ("sk_test_", 16),
    ("sk-", 16),
    ("rk_", 16),
    ("xoxb-", 3),
    ("xoxp-", 3),
    ("AIza", 20),
    ("glpat-", 16),
    ("hf_", 16),
    ("npm_", 16),
    ("pypi-", 16),
];

const SENSITIVE_EXACT: &[&str] = &[
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "PASS",
    "CREDENTIAL",
    "CREDENTIALS",
    "APIKEY",
    "API_KEY",
];

const SENSITIVE_SUFFIXES: &[&str] = &[
    "_KEY",
    "_TOKEN",
    "_SECRET",
    "_PASSWORD",
    "_PASSWD",
    "_CREDENTIAL",
    "_CREDENTIALS",
];

fn scrub_line(line: &str) -> (String, usize) {
    let (mut out, hits) = scrub_prefixed_tokens(line);
    let mut total = hits;
    let (next, hits) = scrub_bearer_tokens(&out);
    out = next;
    total += hits;
    let (next, hits) = scrub_named_assignments(&out);
    out = next;
    total += hits;
    let (next, hits) = scrub_high_entropy(&out);
    out = next;
    total += hits;
    let (next, hits) = scrub_prefixed_alnum(&out, "AKIA", 16, "aws-access-key");
    out = next;
    total += hits;
    (out, total)
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn scrub_token_prefixes(text: &str, prefix: &str, min_tail: usize, label: &str) -> (String, usize) {
    let mut hits = 0;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let replacement = format!("[REDACTED:{label}]");
    while let Some(position) = rest.find(prefix) {
        let (head, tail) = rest.split_at(position);
        out.push_str(head);
        let after = &tail[prefix.len()..];
        let token_len: usize = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
            .map(|c| c.len_utf8())
            .sum();
        if token_len >= min_tail {
            out.push_str(&replacement);
            hits += 1;
            rest = &after[token_len..];
        } else {
            out.push_str(prefix);
            rest = after;
        }
    }
    out.push_str(rest);
    (out, hits)
}

fn scrub_prefixed_tokens(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut hits = 0;
    for (prefix, min_tail) in TOKEN_PREFIXES {
        let (next, found) = scrub_token_prefixes(&out, prefix, *min_tail, "api-token");
        out = next;
        hits += found;
    }
    (out, hits)
}

fn scrub_bearer_tokens(text: &str) -> (String, usize) {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut hits = 0;
    let mut index = 0;
    while index < bytes.len() {
        if matches_bearer_at(bytes, index) {
            let mut cursor = index + "bearer".len();
            cursor = skip_spaces(bytes, cursor);
            if cursor < bytes.len() && bytes[cursor] == b':' {
                cursor = skip_spaces(bytes, cursor + 1);
            } else {
                cursor = skip_spaces(bytes, cursor);
            }
            if cursor < bytes.len() && bytes[cursor] == b'$' {
                out.push_str(&text[index..cursor + 1]);
                index = cursor + 1;
                continue;
            }
            let start = cursor;
            while cursor < bytes.len() && is_bearer_token_byte(bytes[cursor]) {
                cursor += 1;
            }
            if cursor - start >= 8 {
                out.push_str(&text[index..start]);
                out.push_str("[REDACTED:bearer-token]");
                hits += 1;
                index = cursor;
                continue;
            }
            out.push_str(&text[index..cursor.max(index + 1)]);
            index = cursor.max(index + 1);
            continue;
        }
        let next = text[index..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(1);
        out.push_str(&text[index..index + next]);
        index += next;
    }
    (out, hits)
}

fn matches_bearer_at(bytes: &[u8], index: usize) -> bool {
    const WORD: &[u8] = b"bearer";
    if index > 0 && bytes[index - 1].is_ascii_alphanumeric() {
        return false;
    }
    if bytes.len() < index + WORD.len() {
        return false;
    }
    if !bytes[index..index + WORD.len()].eq_ignore_ascii_case(WORD) {
        return false;
    }
    match bytes.get(index + WORD.len()) {
        None => false,
        Some(next) => !next.is_ascii_alphanumeric() || *next == b':' || *next == b' ',
    }
}

fn skip_spaces(bytes: &[u8], mut cursor: usize) -> usize {
    while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
        cursor += 1;
    }
    cursor
}

fn is_bearer_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-')
}

fn is_sensitive_name(normalized: &str) -> bool {
    if SENSITIVE_EXACT.contains(&normalized) {
        return true;
    }
    SENSITIVE_SUFFIXES
        .iter()
        .any(|suffix| normalized.len() > suffix.len() && normalized.ends_with(suffix))
}

fn normalize_name(raw: &str) -> String {
    raw.trim_start_matches('-').replace('-', "_").to_uppercase()
}

fn scrub_named_assignments(text: &str) -> (String, usize) {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut hits = 0;
    let mut index = 0;
    while index < bytes.len() {
        let dash_count = count_leading_dashes(bytes, index);
        let cursor = index + dash_count;
        let first = bytes.get(cursor).copied().unwrap_or(0);
        if !(first.is_ascii_alphabetic() || first == b'_') {
            let next = text[index..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            out.push_str(&text[index..index + next]);
            index += next;
            continue;
        }
        let mut end = cursor;
        while end < bytes.len() && is_ident_byte(bytes[end]) {
            end += 1;
        }
        let flagged = dash_count > 0;
        let name = &text[cursor..end];
        if !is_sensitive_name(&normalize_name(name)) {
            out.push_str(&text[index..end]);
            index = end;
            continue;
        }
        let mut after = end;
        after = skip_spaces(bytes, after);
        let mut needs_equals = !flagged;
        if after < bytes.len() && (bytes[after] == b'=' || bytes[after] == b':') {
            after += 1;
            after = skip_spaces(bytes, after);
            needs_equals = false;
        }
        if needs_equals {
            out.push_str(&text[index..end]);
            index = end;
            continue;
        }
        if after >= bytes.len() {
            out.push_str(&text[index..]);
            break;
        }
        let (value_end, has_value) = assignment_value_end(text, after);
        if !has_value {
            out.push_str(&text[index..after]);
            index = after;
            continue;
        }
        out.push_str(&text[index..after]);
        out.push_str("[REDACTED:credential]");
        hits += 1;
        index = value_end;
    }
    (out, hits)
}

fn count_leading_dashes(bytes: &[u8], mut index: usize) -> usize {
    let mut count = 0;
    while index < bytes.len() && bytes[index] == b'-' {
        count += 1;
        index += 1;
    }
    count
}

fn assignment_value_end(text: &str, start: usize) -> (usize, bool) {
    let bytes = text.as_bytes();
    if start >= bytes.len() {
        return (start, false);
    }
    let quote = bytes[start];
    if quote == b'\'' || quote == b'"' {
        let mut cursor = start + 1;
        while cursor < bytes.len() {
            if bytes[cursor] == quote {
                return (cursor + 1, true);
            }
            cursor += text[cursor..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
        }
        return (bytes.len(), true);
    }
    if bytes[start] == b'`' {
        let mut cursor = start + 1;
        while cursor < bytes.len() && bytes[cursor] != b'`' {
            cursor += text[cursor..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
        }
        if cursor < bytes.len() {
            cursor += 1;
        }
        return (cursor, true);
    }
    let mut cursor = start;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if byte == b' ' || byte == b'\t' || byte == b'\n' || byte == b'\r' {
            break;
        }
        if matches!(byte, b',' | b';' | b'"' | b'\'') {
            break;
        }
        if byte == b'\\' {
            cursor += 1;
        }
        cursor += text[cursor..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(1);
    }
    (cursor, cursor > start)
}

fn scrub_high_entropy(text: &str) -> (String, usize) {
    const KEYWORDS: &[&str] = &[
        "key",
        "token",
        "secret",
        "password",
        "passwd",
        "bearer",
        "auth",
        "credential",
        "apikey",
        "api_key",
    ];
    let bytes = text.as_bytes();
    let lower = text.to_lowercase();
    let lower_bytes = lower.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut hits = 0;
    let mut index = 0;
    while index < bytes.len() {
        let mut keyword_end = None;
        for keyword in KEYWORDS {
            let key = keyword.as_bytes();
            if index + key.len() <= lower_bytes.len()
                && &lower_bytes[index..index + key.len()] == key
                && (index == 0
                    || (!lower_bytes[index - 1].is_ascii_alphanumeric()
                        && lower_bytes[index - 1] != b'_'))
                && (index + key.len() >= lower_bytes.len()
                    || (!lower_bytes[index + key.len()].is_ascii_alphanumeric()
                        && lower_bytes[index + key.len()] != b'_'))
            {
                keyword_end = Some(index + key.len());
                break;
            }
        }
        let Some(mut cursor) = keyword_end else {
            let next = text[index..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            out.push_str(&text[index..index + next]);
            index += next;
            continue;
        };
        let mut skipped = 0;
        while cursor < bytes.len()
            && skipped < 16
            && matches!(
                bytes[cursor],
                b' ' | b'\t' | b'=' | b':' | b'"' | b'\'' | b'(' | b'['
            )
        {
            cursor += 1;
            skipped += 1;
        }
        let mut end = cursor;
        while end < bytes.len() && is_entropy_byte(bytes[end]) {
            end += 1;
        }
        if skipped > 0
            && end - cursor >= 32
            && !text[cursor..end].starts_with("[REDACTED")
            && has_entropy_marks(&text[cursor..end])
        {
            out.push_str(&text[index..cursor]);
            out.push_str("[REDACTED:high-entropy]");
            hits += 1;
            index = end;
        } else {
            let next = text[index..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            out.push_str(&text[index..index + next]);
            index += next;
        }
    }
    (out, hits)
}

fn is_entropy_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'.' | b'_' | b'-')
}

fn has_entropy_marks(text: &str) -> bool {
    let mut has_digit = false;
    let mut has_upper_or_special = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            has_digit = true;
        }
        if c.is_ascii_uppercase()
            || c == '+'
            || c == '/'
            || c == '='
            || c == '.'
            || c == '_'
            || c == '-'
        {
            has_upper_or_special = true;
        }
    }
    has_digit && has_upper_or_special
}

fn scrub_prefixed_alnum(text: &str, prefix: &str, tail: usize, label: &str) -> (String, usize) {
    let mut hits = 0;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let replacement = format!("[REDACTED:{label}]");
    while let Some(position) = rest.find(prefix) {
        let (head, tail_text) = rest.split_at(position);
        out.push_str(head);
        let after = &tail_text[prefix.len()..];
        let alnum_len: usize = after
            .chars()
            .take(tail)
            .take_while(|c| c.is_ascii_alphanumeric())
            .map(|c| c.len_utf8())
            .sum();
        if alnum_len == tail {
            out.push_str(&replacement);
            hits += 1;
            rest = &after[alnum_len..];
        } else {
            out.push_str(prefix);
            rest = after;
        }
    }
    out.push_str(rest);
    (out, hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload_of(pairs: Vec<(&str, serde_json::Value)>) -> Vec<(String, serde_json::Value)> {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }

    fn scrubbed(input: &HashMap<String, serde_json::Value>) -> HashMap<String, serde_json::Value> {
        sanitize_payload(input.clone()).value
    }

    fn scrubbed_text(input: &HashMap<String, serde_json::Value>) -> String {
        scrubbed(input)
            .get("notes")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn drops_transcript_blobs() {
        let input = payload_of(vec![
            ("transcript", serde_json::json!("huge blob")),
            ("summary", serde_json::json!("kept")),
        ]);
        let result = sanitize_payload(input.into_iter().collect());
        assert!(!result.value.contains_key("transcript"));
        assert!(result.value.contains_key("summary"));
        assert!(result.redactions >= 1);
    }

    #[test]
    fn scrubs_token_spans() {
        let input = payload_of(vec![(
            "notes",
            serde_json::json!("token ghp_fixture0000000000000000000000 leaked"),
        )]);
        let text = scrubbed_text(&input.into_iter().collect());
        assert!(!text.contains("ghp_fixture0000000000000000000000"));
        assert!(text.contains("[REDACTED:api-token]"));
    }

    #[test]
    fn scrubs_assignment_values() {
        let input = payload_of(vec![(
            "notes",
            serde_json::json!("connect with password=hunter2 now"),
        )]);
        let text = scrubbed_text(&input.into_iter().collect());
        assert!(text.contains("[REDACTED:credential]"));
        assert!(!text.contains("hunter2"));
    }

    #[test]
    fn truncates_long_strings() {
        let long = "x".repeat(5000);
        let input = payload_of(vec![("notes", serde_json::json!(long))]);
        let result = sanitize_payload(input.into_iter().collect());
        let text = result.value["notes"].as_str().expect("string");
        assert_eq!(text.chars().count(), 2000);
    }

    #[test]
    fn keeps_innocent_text_verbatim() {
        let input = payload_of(vec![(
            "summary",
            serde_json::json!("Use Temporal for provisioning workflow"),
        )]);
        let result = sanitize_payload(input.into_iter().collect());
        assert_eq!(
            result.value["summary"].as_str(),
            Some("Use Temporal for provisioning workflow")
        );
        assert_eq!(result.redactions, 0);
    }

    #[test]
    fn leaves_redacted_markers_alone() {
        let input = payload_of(vec![(
            "notes",
            serde_json::json!("Attempted connection with REDACTED credential material"),
        )]);
        let out = sanitize_payload(input.into_iter().collect());
        assert_eq!(out.redactions, 0);
    }

    #[test]
    fn scrubs_opencode_api_key() {
        let key = "oc_sk_000000000000_FAKEfakeFAKEfakeFAKEfakeFAKEfa";
        let input = payload_of(vec![(
            "notes",
            serde_json::json!(format!(
                "K='{key}'; curl https://x.example -H \"Authorization: Bearer $K\""
            )),
        )]);
        let text = scrubbed_text(&input.into_iter().collect());
        assert!(!text.contains("oc_sk_"), "prefix must be scrubbed");
        assert!(
            !text.contains("FAKEfakeFAKE"),
            "token body must be scrubbed"
        );
        assert!(text.contains("[REDACTED:api-token]"));
        assert!(text.contains("$K"), "variable reference is not a leak");
    }

    #[test]
    fn scrubs_export_assignment_with_quotes() {
        let key = "oc_sk_000000000000_FAKEfakeFAKEfakeFAKEfakeFAKEfa";
        let input = payload_of(vec![(
            "notes",
            serde_json::json!(format!(
                "printf 'export LAM_OPENAI_COMPAT_API_KEY=%s\\n' '{key}'"
            )),
        )]);
        let text = scrubbed_text(&input.into_iter().collect());
        assert!(!text.contains(key));
        assert!(!text.contains("FAKEfakeFAKE"));
        assert!(text.contains("REDACTED"));
    }

    #[test]
    fn scrubs_bearer_literal_but_not_variable() {
        let input = payload_of(vec![(
            "notes",
            serde_json::json!("Authorization: Bearer abcDEF1234567890xyz"),
        )]);
        let text = scrubbed_text(&input.into_iter().collect());
        assert!(!text.contains("abcDEF1234567890xyz"));
        let kept = payload_of(vec![(
            "notes",
            serde_json::json!("Authorization: Bearer $K"),
        )]);
        assert!(scrubbed_text(&kept.into_iter().collect()).contains("$K"));
    }

    #[test]
    fn scrubs_flag_values_and_quoted_assignments() {
        for raw in [
            "--api-key abcDEF1234567890xyz",
            "--api-key=abcDEF1234567890xyz",
            "api_key='abcDEF1234567890xyz'",
            "TOKEN=\"abcDEF1234567890xyz\"",
        ] {
            let input = payload_of(vec![("notes", serde_json::json!(raw))]);
            let text = scrubbed_text(&input.into_iter().collect());
            assert!(
                !text.contains("abcDEF1234567890xyz"),
                "value must be scrubbed in {raw}"
            );
            assert!(text.contains("REDACTED"), "marker missing in {raw}");
        }
    }

    #[test]
    fn scrubs_known_provider_prefixes() {
        for raw in [
            "token sk-ant-abcdefghijklmnopqrst leaked",
            "key sk_test_1234567890abcdefghij here",
            "use AIzaSyAbcDefGhIjKlMnOpQrStUvWx012345 here",
            "login glpat-abcdefghijklmnopqr12 here",
            "model hf_abcdefghijklmnop1234567890 here",
            "npm_abcdefghijklmnop1234567890 token",
            "pypi-AgEIabcdefghijklmnop1234567890 here",
            "key rk_abcdefghijklmnop1234567890 here",
        ] {
            let input = payload_of(vec![("notes", serde_json::json!(raw))]);
            let text = scrubbed_text(&input.into_iter().collect());
            assert!(
                text.contains("[REDACTED:api-token]"),
                "prefix must be scrubbed in {raw}: {text}"
            );
        }
    }

    #[test]
    fn scrubs_high_entropy_secret_after_keyword() {
        let input = payload_of(vec![(
            "notes",
            serde_json::json!("token abcDEF1234567890xyzXYZ0987654321QW extra"),
        )]);
        let text = scrubbed_text(&input.into_iter().collect());
        assert!(!text.contains("abcDEF1234567890xyzXYZ0987654321QW"));
        assert!(text.contains("[REDACTED:high-entropy]"));
    }

    #[test]
    fn keeps_ordinary_prose_verbatim() {
        for raw in [
            "Use Temporal for provisioning workflow",
            "the monkey typed notes all day",
            "password: hunter2 is only an example here",
        ] {
            let input = payload_of(vec![("notes", serde_json::json!(raw))]);
            let text = scrubbed_text(&input.into_iter().collect());
            if raw.contains("password") {
                assert!(text.contains("REDACTED"));
                assert!(!text.contains("hunter2"));
            } else {
                assert_eq!(text, raw);
            }
        }
    }
}
