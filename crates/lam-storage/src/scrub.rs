pub struct ScrubReport {
    pub scanned: usize,
    pub changed: usize,
}

pub fn scrub_stored_events(
    conn: &mut rusqlite::Connection,
    dry_run: bool,
) -> Result<ScrubReport, lam_core::LamError> {
    let mut query = conn
        .prepare("SELECT id, sanitized_payload FROM session_events")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows: Vec<(String, String)> = query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .collect::<Result<Vec<(String, String)>, _>>()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    drop(query);
    let mut report = ScrubReport {
        scanned: rows.len(),
        changed: 0,
    };
    let mut pending: Vec<(String, String)> = Vec::new();
    for (id, raw) in &rows {
        match scrub_payload(raw) {
            Some(clean) if clean != *raw => {
                report.changed += 1;
                if !dry_run {
                    pending.push((id.clone(), clean));
                }
            }
            _ => {}
        }
    }
    if !dry_run {
        let tx = conn
            .transaction()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        for (id, clean) in &pending {
            tx.execute(
                "UPDATE session_events SET sanitized_payload = ?1 WHERE id = ?2",
                rusqlite::params![clean, id],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        }
        tx.commit()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if !pending.is_empty() {
            crate::db::harden_current_db(conn);
        }
    }
    Ok(report)
}

fn scrub_payload(raw: &str) -> Option<String> {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(mut value) => {
            if scrub_value(&mut value) == 0 {
                return None;
            }
            serde_json::to_string(&value).ok()
        }
        Err(_) => {
            let (cleaned, hits) = lam_adapters::scrub_text(raw);
            if hits == 0 || cleaned == raw {
                None
            } else {
                Some(cleaned)
            }
        }
    }
}

fn scrub_value(value: &mut serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(text) => {
            let (cleaned, hits) = lam_adapters::scrub_text(text);
            if hits > 0 {
                *text = cleaned;
            }
            hits
        }
        serde_json::Value::Array(items) => items.iter_mut().map(scrub_value).sum(),
        serde_json::Value::Object(map) => map.values_mut().map(scrub_value).sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn seeded_conn() -> rusqlite::Connection {
        let mut conn = open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        conn.execute(
            "INSERT INTO projects (id, name, canonical_root, identity_hash, created_at) VALUES ('p1', 'demo', '/tmp/demo', 'h', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("project");
        conn.execute(
            "INSERT INTO sessions (id, project_id, agent_kind, external_session_id, started_at) VALUES ('s1', 'p1', 'cli', 'ext-1', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("session");
        let leaked = serde_json::json!({
            "tool_input": {
                "command": "K='oc_sk_000000000000_FAKEfakeFAKEfakeFAKEfakeFAKEfa'; curl https://x.example"
            }
        })
        .to_string();
        conn.execute(
            "INSERT INTO session_events (id, session_id, event_key, kind, sanitized_payload, created_at) VALUES ('e1', 's1', 'k1', 'tool.use', ?1, '2026-01-01T00:00:00Z')",
            rusqlite::params![leaked],
        )
        .expect("leaked event");
        let clean = serde_json::json!({"tool_input": {"command": "cargo test"}}).to_string();
        conn.execute(
            "INSERT INTO session_events (id, session_id, event_key, kind, sanitized_payload, created_at) VALUES ('e2', 's1', 'k2', 'tool.use', ?1, '2026-01-01T00:00:00Z')",
            rusqlite::params![clean],
        )
        .expect("clean event");
        conn
    }

    #[test]
    fn dry_run_changes_nothing() {
        let mut conn = seeded_conn();
        let report = scrub_stored_events(&mut conn, true).expect("scrub");
        assert_eq!(report.scanned, 2);
        assert_eq!(report.changed, 1);
        let raw: String = conn
            .query_row(
                "SELECT sanitized_payload FROM session_events WHERE id = 'e1'",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert!(raw.contains("oc_sk_000000000000"), "dry run keeps bytes");
    }

    #[test]
    fn scrub_rewrites_leaked_payload() {
        let mut conn = seeded_conn();
        let report = scrub_stored_events(&mut conn, false).expect("scrub");
        assert_eq!(report.scanned, 2);
        assert_eq!(report.changed, 1);
        let raw: String = conn
            .query_row(
                "SELECT sanitized_payload FROM session_events WHERE id = 'e1'",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert!(!raw.contains("oc_sk_"), "leak must be gone");
        assert!(raw.contains("[REDACTED:api-token]"));
        let clean: String = conn
            .query_row(
                "SELECT sanitized_payload FROM session_events WHERE id = 'e2'",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert!(clean.contains("cargo test"));
    }

    #[test]
    fn scrub_is_idempotent() {
        let mut conn = seeded_conn();
        scrub_stored_events(&mut conn, false).expect("first");
        let report = scrub_stored_events(&mut conn, false).expect("second");
        assert_eq!(report.changed, 0);
    }
}
