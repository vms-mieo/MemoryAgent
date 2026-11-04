use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct RemoteCall {
    pub provider: String,
    pub model: String,
    pub endpoint_host: String,
    pub payload_bytes: i64,
    pub redactions: i64,
    pub status: String,
}

pub fn record_remote_call(
    conn: &rusqlite::Connection,
    call: &RemoteCall,
    now: DateTime<Utc>,
) -> Result<String, lam_core::LamError> {
    let id = format!("remote-{}", lam_core::RevisionId::new());
    conn.execute(
        "INSERT INTO remote_calls (id, provider, model, endpoint_host, payload_bytes, redactions, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            id,
            call.provider.trim(),
            call.model.trim(),
            call.endpoint_host.trim(),
            call.payload_bytes.max(0),
            call.redactions.max(0),
            call.status.trim(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(id)
}

pub fn remote_call_count(conn: &rusqlite::Connection) -> Result<i64, lam_core::LamError> {
    conn.query_row("SELECT COUNT(*) FROM remote_calls", [], |row| row.get(0))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_calls_record_without_content() {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        let now = chrono::Utc::now();
        record_remote_call(
            &conn,
            &RemoteCall {
                provider: "openai-compatible".to_string(),
                model: "muse-spark".to_string(),
                endpoint_host: "opencode.ai".to_string(),
                payload_bytes: 1024,
                redactions: 2,
                status: "ok".to_string(),
            },
            now,
        )
        .expect("record");
        assert_eq!(remote_call_count(&conn).expect("count"), 1);
    }
}
