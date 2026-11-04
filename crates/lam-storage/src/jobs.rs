use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    EpisodeSummarize,
    FactExtract,
    Embed,
    GraphExtract,
    DedupScan,
    DreamBatch,
    ReviewPrepare,
    VacuumCleanup,
    Reindex,
}

impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::EpisodeSummarize => "episode_summarize",
            JobKind::FactExtract => "fact_extract",
            JobKind::Embed => "embed",
            JobKind::GraphExtract => "graph_extract",
            JobKind::DedupScan => "dedup_scan",
            JobKind::DreamBatch => "dream_batch",
            JobKind::ReviewPrepare => "review_prepare",
            JobKind::VacuumCleanup => "vacuum_cleanup",
            JobKind::Reindex => "reindex",
        }
    }

    pub fn all() -> &'static [JobKind] {
        &[
            JobKind::EpisodeSummarize,
            JobKind::FactExtract,
            JobKind::Embed,
            JobKind::GraphExtract,
            JobKind::DedupScan,
            JobKind::DreamBatch,
            JobKind::ReviewPrepare,
            JobKind::VacuumCleanup,
            JobKind::Reindex,
        ]
    }
}

impl std::str::FromStr for JobKind {
    type Err = lam_core::LamError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "episode_summarize" => Ok(JobKind::EpisodeSummarize),
            "fact_extract" => Ok(JobKind::FactExtract),
            "embed" => Ok(JobKind::Embed),
            "graph_extract" => Ok(JobKind::GraphExtract),
            "dedup_scan" => Ok(JobKind::DedupScan),
            "dream_batch" => Ok(JobKind::DreamBatch),
            "review_prepare" => Ok(JobKind::ReviewPrepare),
            "vacuum_cleanup" => Ok(JobKind::VacuumCleanup),
            "reindex" => Ok(JobKind::Reindex),
            _ => Err(lam_core::LamError::validation(
                "E_JOB_KIND_INVALID",
                format!("unknown job kind: {s}"),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub input_ref: String,
    pub state: String,
    pub attempts: i64,
    pub due_at: DateTime<Utc>,
    pub lease_until: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

fn row_to_job(row: &rusqlite::Row) -> Result<Job, rusqlite::Error> {
    let due_text: String = row.get(5)?;
    let lease_text: Option<String> = row.get(6)?;
    Ok(Job {
        id: row.get(0)?,
        kind: row.get(1)?,
        input_ref: row.get(2)?,
        state: row.get(3)?,
        attempts: row.get(4)?,
        due_at: due_text.parse::<DateTime<Utc>>().map_err(|_| {
            rusqlite::Error::InvalidColumnType(5, "due_at".to_string(), rusqlite::types::Type::Text)
        })?,
        lease_until: lease_text
            .map(|text| {
                text.parse::<DateTime<Utc>>().map_err(|_| {
                    rusqlite::Error::InvalidColumnType(
                        6,
                        "lease_until".to_string(),
                        rusqlite::types::Type::Text,
                    )
                })
            })
            .transpose()?,
        error: row.get(7)?,
    })
}

pub fn is_terminal(state: &str) -> bool {
    matches!(state, "done" | "dead" | "cancelled")
}

pub fn enqueue(
    conn: &rusqlite::Connection,
    kind: &str,
    input_ref: &str,
    due_in_ms: u64,
    now: DateTime<Utc>,
) -> Result<String, lam_core::LamError> {
    kind.parse::<JobKind>()?;
    if input_ref.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_JOB_REF_INVALID",
            "job input_ref must not be empty".to_string(),
        ));
    }
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM maintenance_jobs WHERE kind = ?1 AND input_ref = ?2 AND state IN ('pending', 'leased')",
            rusqlite::params![kind.trim(), input_ref.trim()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = format!("job-{}", lam_core::RevisionId::new());
    let due_at = now + chrono::Duration::milliseconds(due_in_ms.min(3_600_000) as i64);
    conn.execute(
        "INSERT INTO maintenance_jobs (id, kind, input_ref, state, attempts, due_at, lease_until, error)
         VALUES (?1, ?2, ?3, 'pending', 0, ?4, NULL, NULL)",
        rusqlite::params![id, kind.trim(), input_ref.trim(), due_at.to_rfc3339()],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(id)
}

pub fn lease_next(
    conn: &mut rusqlite::Connection,
    worker: &str,
    now: DateTime<Utc>,
    lease_ms: u64,
    worker_note: &str,
) -> Result<Option<Job>, lam_core::LamError> {
    if worker.trim().is_empty() {
        return Err(lam_core::LamError::validation(
            "E_WORKER_INVALID",
            "worker id must not be empty".to_string(),
        ));
    }
    let _ = worker_note;
    let tx = conn
        .transaction()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let candidate: Option<String> = tx
        .query_row(
            "SELECT id FROM maintenance_jobs
             WHERE (state = 'pending' AND due_at <= ?1)
                OR (state = 'leased' AND lease_until IS NOT NULL AND lease_until < ?1)
             ORDER BY due_at ASC LIMIT 1",
            rusqlite::params![now.to_rfc3339()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let id = match candidate {
        Some(found) => found,
        None => {
            tx.rollback()
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            return Ok(None);
        }
    };
    let lease_until = now + chrono::Duration::milliseconds(lease_ms.clamp(1_000, 3_600_000) as i64);
    tx.execute(
        "UPDATE maintenance_jobs SET state = 'leased', lease_until = ?1, attempts = attempts + 1, error = NULL WHERE id = ?2",
        rusqlite::params![lease_until.to_rfc3339(), id],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let job: Job = tx
        .query_row(
            "SELECT id, kind, input_ref, state, attempts, due_at, lease_until, error FROM maintenance_jobs WHERE id = ?1",
            rusqlite::params![id],
            row_to_job,
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.commit()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    let _ = worker;
    Ok(Some(job))
}

pub fn heartbeat(
    conn: &rusqlite::Connection,
    job_id: &str,
    extend_ms: u64,
    now: DateTime<Utc>,
) -> Result<bool, lam_core::LamError> {
    let lease_until =
        now + chrono::Duration::milliseconds(extend_ms.clamp(1_000, 3_600_000) as i64);
    let changed = conn
        .execute(
            "UPDATE maintenance_jobs SET lease_until = ?1 WHERE id = ?2 AND state = 'leased'",
            rusqlite::params![lease_until.to_rfc3339(), job_id.trim()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(changed == 1)
}

pub fn complete(
    conn: &rusqlite::Connection,
    job_id: &str,
    worker: &str,
) -> Result<bool, lam_core::LamError> {
    let _ = worker;
    let changed = conn
        .execute(
            "UPDATE maintenance_jobs SET state = 'done', lease_until = NULL, error = NULL WHERE id = ?1 AND state = 'leased'",
            rusqlite::params![job_id.trim()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if changed == 1 {
        crate::db::harden_current_db(conn);
    }
    Ok(changed == 1)
}

pub fn fail(
    conn: &rusqlite::Connection,
    job_id: &str,
    error: &str,
    retryable: bool,
    max_attempts: u32,
    now: DateTime<Utc>,
) -> Result<bool, lam_core::LamError> {
    let job: Option<Job> = conn
        .query_row(
            "SELECT id, kind, input_ref, state, attempts, due_at, lease_until, error FROM maintenance_jobs WHERE id = ?1",
            rusqlite::params![job_id.trim()],
            row_to_job,
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let Some(current) = job else {
        return Ok(false);
    };
    if current.state != "leased" {
        return Ok(false);
    }
    let safe_error: String = error.chars().take(2000).collect();
    if retryable && (current.attempts as u32) < max_attempts.max(1) {
        let backoff_ms = (30_000u64.saturating_mul(1u64 << current.attempts.min(6))).min(3_600_000);
        let due_at = now + chrono::Duration::milliseconds(backoff_ms as i64);
        conn.execute(
            "UPDATE maintenance_jobs SET state = 'pending', lease_until = NULL, error = ?1, due_at = ?2 WHERE id = ?3",
            rusqlite::params![safe_error, due_at.to_rfc3339(), current.id],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    } else {
        conn.execute(
            "UPDATE maintenance_jobs SET state = 'dead', lease_until = NULL, error = ?1 WHERE id = ?2",
            rusqlite::params![safe_error, current.id],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    }
    crate::db::harden_current_db(conn);
    Ok(true)
}

pub fn cancel(conn: &rusqlite::Connection, job_id: &str) -> Result<bool, lam_core::LamError> {
    let changed = conn
        .execute(
            "UPDATE maintenance_jobs SET state = 'cancelled', lease_until = NULL WHERE id = ?1 AND state IN ('pending', 'leased')",
            rusqlite::params![job_id.trim()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(changed == 1)
}

pub fn release(
    conn: &rusqlite::Connection,
    job_id: &str,
    note: &str,
) -> Result<bool, lam_core::LamError> {
    let safe_note: String = note.chars().take(2000).collect();
    let changed = conn
        .execute(
            "UPDATE maintenance_jobs SET state = 'pending', lease_until = NULL, attempts = CASE WHEN attempts > 0 THEN attempts - 1 ELSE 0 END, error = ?1 WHERE id = ?2 AND state = 'leased'",
            rusqlite::params![safe_note, job_id.trim()],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if changed == 1 {
        crate::db::harden_current_db(conn);
    }
    Ok(changed == 1)
}

pub fn counts_by_state(
    conn: &rusqlite::Connection,
) -> Result<Vec<(String, i64)>, lam_core::LamError> {
    let mut stmt = conn
        .prepare("SELECT state, COUNT(*) FROM maintenance_jobs GROUP BY state ORDER BY state ASC")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| lam_core::LamError::storage(e.to_string()))?);
    }
    Ok(out)
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0)
            .single()
            .expect("fixed time")
    }

    #[test]
    fn enqueue_is_idempotent_per_kind_and_ref() {
        let conn = migrated_conn();
        let first = enqueue(&conn, "dream_batch", "proj:topic:42", 0, at()).expect("enqueue");
        let second = enqueue(&conn, "dream_batch", "proj:topic:42", 0, at()).expect("re-enqueue");
        assert_eq!(first, second);
        let other = enqueue(&conn, "dream_batch", "proj:other:1", 0, at()).expect("other");
        assert_ne!(first, other);
    }

    #[test]
    fn lease_complete_cycle() {
        let mut conn = migrated_conn();
        let id = enqueue(&conn, "fact_extract", "session:s1", 0, at()).expect("enqueue");
        let job = lease_next(&mut conn, "worker-1", at(), 60_000, "")
            .expect("lease")
            .expect("job");
        assert_eq!(job.id, id);
        assert_eq!(job.state, "leased");
        assert_eq!(job.attempts, 1);
        assert!(complete(&conn, &id, "worker-1").expect("complete"));
        assert!(lease_next(&mut conn, "worker-1", at(), 60_000, "")
            .expect("lease")
            .is_none());
    }

    #[test]
    fn expired_lease_is_recoverable_without_duplicates() {
        let mut conn = migrated_conn();
        let id = enqueue(&conn, "embed", "doc:d1", 0, at()).expect("enqueue");
        let first = lease_next(&mut conn, "worker-1", at(), 60_000, "")
            .expect("lease")
            .expect("job");
        assert_eq!(first.id, id);
        conn.execute(
            "UPDATE maintenance_jobs SET lease_until = '2020-01-01T00:00:00+00:00' WHERE id = ?1",
            rusqlite::params![id],
        )
        .expect("expire lease");
        let second = lease_next(&mut conn, "worker-2", at(), 60_000, "")
            .expect("re-lease")
            .expect("job");
        assert_eq!(second.id, id);
        assert_eq!(second.attempts, 2);
        assert!(complete(&conn, &id, "worker-2").expect("complete"));
        let done: Vec<(String, i64)> = counts_by_state(&conn).expect("counts");
        assert!(done
            .iter()
            .any(|(state, count)| state == "done" && *count == 1));
    }

    #[test]
    fn failures_back_off_then_die() {
        let conn = migrated_conn();
        let id = enqueue(&conn, "vacuum_cleanup", "global", 0, at()).expect("enqueue");
        let mut owned = conn;
        let first = lease_next(&mut owned, "w", at(), 60_000, "")
            .expect("lease")
            .expect("job");
        assert!(fail(&owned, &first.id, "boom", true, 2, at()).expect("fail"));
        let second = lease_next(&mut owned, "w", at(), 60_000, "").expect("lease");
        assert!(second.is_none(), "backoff must defer the retry");
        let row: String = owned
            .query_row(
                "SELECT state FROM maintenance_jobs WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .expect("state");
        assert_eq!(row, "pending");
    }

    #[test]
    fn unknown_kind_rejected() {
        let conn = migrated_conn();
        let result = enqueue(&conn, "teleport", "x", 0, at());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "E_JOB_KIND_INVALID");
    }
}
