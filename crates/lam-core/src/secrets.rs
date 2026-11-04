const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("oc_sk_", 8),
    ("ghp_", 4),
    ("gho_", 4),
    ("github_pat_", 4),
    ("sk-ant-", 4),
    ("sk-proj-", 4),
    ("sk_live_", 8),
    ("sk_test_", 8),
    ("sk-", 12),
    ("rk_", 12),
    ("xoxb-", 3),
    ("xoxp-", 3),
    ("AIza", 16),
    ("glpat-", 12),
    ("hf_", 12),
    ("npm_", 12),
    ("pypi-", 12),
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

const ENTROPY_KEYWORDS: &[&str] = &[
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

pub fn find_secret_kind(content: &str) -> Option<&'static str> {
    let live: Vec<&str> = content
        .lines()
        .filter(|line| !line.to_lowercase().contains("redacted"))
        .collect();
    if live.iter().any(|line| contains_private_key(line)) {
        return Some("private-key");
    }
    if live
        .iter()
        .any(|line| contains_prefixed_token(line, "AKIA", 16))
    {
        return Some("aws-access-key");
    }
    for line in &live {
        for (prefix, minimum) in TOKEN_PREFIXES {
            if contains_prefixed_token(line, prefix, *minimum) {
                return Some("api-token");
            }
        }
        if contains_bearer_token(line) {
            return Some("api-token");
        }
        if contains_sensitive_assignment(line) {
            return Some("credential-assignment");
        }
        if contains_high_entropy_secret(line) {
            return Some("api-token");
        }
    }
    None
}

pub fn ensure_no_secrets(content: &str) -> Result<(), crate::error::LamError> {
    if let Some(kind) = find_secret_kind(content) {
        return Err(crate::error::LamError::validation(
            "E_SECRET_DETECTED",
            format!("content looks like a {kind}; refusing to store"),
        ));
    }
    Ok(())
}

fn contains_private_key(content: &str) -> bool {
    content.contains("PRIVATE KEY")
}

fn contains_prefixed_token(content: &str, prefix: &str, tail: usize) -> bool {
    let bytes = content.as_bytes();
    let mark = prefix.as_bytes();
    if bytes.len() < mark.len() + tail {
        return false;
    }
    for start in 0..=(bytes.len() - mark.len() - tail) {
        if &bytes[start..start + mark.len()] == mark {
            let after = &bytes[start + mark.len()..];
            let run: usize = after
                .iter()
                .take_while(|b| {
                    b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-' || **b == b'.'
                })
                .count();
            if run >= tail {
                return true;
            }
        }
    }
    false
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn normalize_assignment_name(raw: &str) -> String {
    raw.trim_start_matches('-').replace('-', "_").to_uppercase()
}

fn is_sensitive_name(normalized: &str) -> bool {
    if SENSITIVE_EXACT.contains(&normalized) {
        return true;
    }
    SENSITIVE_SUFFIXES
        .iter()
        .any(|suffix| normalized.len() > suffix.len() && normalized.ends_with(suffix))
}

fn contains_sensitive_assignment(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let mut cursor = index;
        while cursor < bytes.len() && bytes[cursor] == b'-' {
            cursor += 1;
        }
        let first = bytes.get(cursor).copied().unwrap_or(0);
        if !(first.is_ascii_alphabetic() || first == b'_') {
            index += 1;
            continue;
        }
        let mut end = cursor;
        while end < bytes.len() && is_ident_byte(bytes[end]) {
            end += 1;
        }
        let flagged = cursor > index;
        let name = &line[cursor..end];
        if is_sensitive_name(&normalize_assignment_name(name)) {
            let mut after = end;
            while after < bytes.len() && (bytes[after] == b' ' || bytes[after] == b'\t') {
                after += 1;
            }
            if after < bytes.len() && (bytes[after] == b'=' || bytes[after] == b':') {
                after += 1;
                while after < bytes.len() && (bytes[after] == b' ' || bytes[after] == b'\t') {
                    after += 1;
                }
                if after < bytes.len() {
                    return true;
                }
            } else if flagged && after < bytes.len() {
                return true;
            }
        }
        index = end.max(index + 1);
    }
    false
}

fn contains_bearer_token(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if matches_bearer_at(bytes, index) {
            let mut cursor = index + "bearer".len();
            while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
                cursor += 1;
            }
            if cursor < bytes.len() && bytes[cursor] == b':' {
                cursor += 1;
                while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
                    cursor += 1;
                }
            }
            if cursor < bytes.len() && bytes[cursor] == b'$' {
                index = cursor + 1;
                continue;
            }
            let mut length = 0;
            while cursor + length < bytes.len() && is_bearer_token_byte(bytes[cursor + length]) {
                length += 1;
            }
            if length >= 8 {
                return true;
            }
            index = cursor.max(index + 1);
            continue;
        }
        index += 1;
    }
    false
}

fn matches_bearer_at(bytes: &[u8], index: usize) -> bool {
    const WORD: &[u8] = b"bearer";
    if index > 0 && bytes[index - 1].is_ascii_alphanumeric() {
        return false;
    }
    if bytes.len() < index + WORD.len() {
        return false;
    }
    bytes[index..index + WORD.len()].eq_ignore_ascii_case(WORD)
}

fn is_bearer_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-')
}

fn contains_high_entropy_secret(line: &str) -> bool {
    let bytes = line.as_bytes();
    let lower = line.to_lowercase();
    let lower_bytes = lower.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let mut keyword_end = None;
        for keyword in ENTROPY_KEYWORDS {
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
            index += 1;
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
        if skipped > 0 && end - cursor >= 32 && !line[cursor..end].starts_with("[REDACTED") {
            let run = &line[cursor..end];
            if run.bytes().any(|b| b.is_ascii_digit())
                && run.bytes().any(|b| {
                    (b as char).is_ascii_uppercase()
                        || matches!(b, b'+' | b'/' | b'=' | b'.' | b'_' | b'-')
                })
            {
                return true;
            }
        }
        index += 1;
    }
    false
}

fn is_entropy_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'.' | b'_' | b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_aws_key_shape() {
        assert_eq!(
            find_secret_kind("key AKIAIOSFODNN7EXAMPLE extra"),
            Some("aws-access-key")
        );
        assert!(ensure_no_secrets("key AKIAIOSFODNN7EXAMPLE extra").is_err());
    }
    #[test]
    fn rejects_private_key_block() {
        let block = "-----BEGIN RSA PRIVATE KEY-----\nabc";
        assert_eq!(find_secret_kind(block), Some("private-key"));
    }
    #[test]
    fn rejects_token_prefixes() {
        assert_eq!(find_secret_kind("token ghp_abc123"), Some("api-token"));
        assert_eq!(find_secret_kind("hook xoxb-123"), Some("api-token"));
    }
    #[test]
    fn accepts_ordinary_prose() {
        assert_eq!(
            find_secret_kind("Use Temporal for provisioning workflow"),
            None
        );
        assert!(ensure_no_secrets("Use Temporal for provisioning workflow").is_ok());
    }
    #[test]
    fn accepts_redacted_fixture_text() {
        let text = "Attempted connection with REDACTED credential material";
        assert_eq!(find_secret_kind(text), None);
    }
    #[test]
    fn rejects_opencode_api_key() {
        let key = "oc_sk_000000000000_FAKEfakeFAKEfakeFAKEfakeFAKEfa";
        assert_eq!(
            find_secret_kind(&format!("K='{key}'; curl https://x.example")),
            Some("api-token")
        );
    }
    #[test]
    fn rejects_more_provider_prefixes() {
        for sample in [
            "token sk-ant-abcdefghijklmnopqrst",
            "key sk_test_1234567890abcdefghij",
            "use AIzaSyAbcDefGhIjKlMnOpQrStUvWx012345 here",
            "login glpat-abcdefghijklmnopqr12 ok",
            "model hf_abcdefghijklmnop1234567890 here",
            "npm_abcdefghijklmnop1234567890 token",
            "pypi-AgEIabcdefghijklmnop1234567890 here",
            "key rk_abcdefghijklmnop1234567890 here",
        ] {
            assert_eq!(find_secret_kind(sample), Some("api-token"), "{sample}");
        }
    }
    #[test]
    fn rejects_bearer_and_assignments() {
        assert_eq!(
            find_secret_kind("Authorization: Bearer abcDEF1234567890xyz"),
            Some("api-token")
        );
        assert_eq!(
            find_secret_kind("api_key='abcDEF1234567890xyz'"),
            Some("credential-assignment")
        );
        assert_eq!(
            find_secret_kind("--api-key abcDEF1234567890xyz"),
            Some("credential-assignment")
        );
        assert_eq!(find_secret_kind("Authorization: Bearer $K"), None);
    }
    #[test]
    fn rejects_high_entropy_after_keyword() {
        assert_eq!(
            find_secret_kind("token abcDEF1234567890xyzXYZ0987654321QW extra"),
            Some("api-token")
        );
    }
    #[test]
    fn keeps_innocent_text_verbatim() {
        for text in [
            "Use Temporal for provisioning workflow",
            "the monkey typed notes all day",
            "deploy revision three to staging",
        ] {
            assert_eq!(find_secret_kind(text), None, "{text}");
        }
    }
}
