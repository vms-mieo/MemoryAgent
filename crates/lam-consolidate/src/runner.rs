use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct MaintainOutcome {
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub deferred: usize,
    pub budget_exceeded: bool,
    pub elapsed_ms: u64,
}

pub fn maintain(
    conn: &mut rusqlite::Connection,
    worker: &str,
    budget_ms: u64,
    only_kind: Option<&str>,
    handle: &dyn Fn(
        &mut rusqlite::Connection,
        &lam_storage::Job,
    ) -> Result<String, lam_core::LamError>,
) -> MaintainOutcome {
    let started = Instant::now();
    let budget = std::time::Duration::from_millis(budget_ms.max(1));
    let mut outcome = MaintainOutcome::default();
    let mut deferred_ids: Vec<String> = Vec::new();
    loop {
        if started.elapsed() >= budget {
            outcome.budget_exceeded = true;
            break;
        }
        let leased = match lam_storage::lease_next(conn, worker, chrono::Utc::now(), 120_000, "") {
            Ok(job) => job,
            Err(_) => break,
        };
        let Some(job) = leased else {
            break;
        };
        if deferred_ids.iter().any(|id| id == &job.id) {
            let _ = lam_storage::release(conn, &job.id, "already deferred this run");
            break;
        }
        if let Some(kind) = only_kind {
            if job.kind != kind {
                let _ = lam_storage::fail(
                    conn,
                    &job.id,
                    "skipped by kind filter",
                    false,
                    1,
                    chrono::Utc::now(),
                );
                continue;
            }
        }
        outcome.processed += 1;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(conn, &job)));
        match result {
            Ok(Ok(_)) => {
                if lam_storage::complete(conn, &job.id, worker).unwrap_or(false) {
                    outcome.succeeded += 1;
                }
            }
            Ok(Err(e)) if e.code() == "E_REMOTE_DEFERRED" => {
                outcome.deferred += 1;
                deferred_ids.push(job.id.clone());
                let _ = lam_storage::release(conn, &job.id, &e.to_string());
            }
            Ok(Err(e)) => {
                outcome.failed += 1;
                let _ =
                    lam_storage::fail(conn, &job.id, &e.to_string(), true, 5, chrono::Utc::now());
            }
            Err(_) => {
                outcome.failed += 1;
                let _ = lam_storage::fail(
                    conn,
                    &job.id,
                    "job handler panicked",
                    false,
                    1,
                    chrono::Utc::now(),
                );
            }
        }
    }
    outcome.elapsed_ms = started.elapsed().as_millis() as u64;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = lam_storage::open_in_memory(3000).expect("open memory");
        lam_storage::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    fn at() -> chrono::DateTime<chrono::Utc> {
        chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 11, 2, 9, 0, 0)
            .single()
            .expect("fixed time")
    }

    #[test]
    fn empty_queue_is_noop_within_budget() {
        let mut conn = migrated_conn();
        let outcome = maintain(&mut conn, "w", 1000, None, &|_, _| Ok("ok".to_string()));
        assert_eq!(outcome.processed, 0);
        assert!(!outcome.budget_exceeded);
    }

    #[test]
    fn failing_handler_goes_through_retry_then_dead() {
        let mut conn = migrated_conn();
        lam_storage::enqueue(&conn, "vacuum_cleanup", "global", 0, at()).expect("enqueue");
        let handler = |_: &mut rusqlite::Connection,
                       _: &lam_storage::Job|
         -> Result<String, lam_core::LamError> {
            Err(lam_core::LamError::storage("boom".to_string()))
        };
        for _ in 0..6 {
            maintain(&mut conn, "w", 5000, None, &handler);
            conn.execute(
                "UPDATE maintenance_jobs SET due_at = '2020-01-01T00:00:00+00:00' WHERE state = 'pending'",
                [],
            )
            .expect("rewind due");
        }
        let counts = lam_storage::counts_by_state(&conn).expect("counts");
        let dead = counts
            .iter()
            .find(|(state, _)| state == "dead")
            .map(|(_, count)| *count)
            .unwrap_or(0);
        let pending = counts
            .iter()
            .find(|(state, _)| state == "pending")
            .map(|(_, count)| *count)
            .unwrap_or(0);
        assert_eq!(dead, 1);
        assert_eq!(pending, 0);
    }

    #[test]
    fn deferred_remote_job_releases_lease_and_stays_pending() {
        let mut conn = migrated_conn();
        lam_storage::enqueue(&conn, "fact_extract", "session:s-1", 0, at()).expect("enqueue");
        conn.execute(
            "UPDATE maintenance_jobs SET due_at = '2020-01-01T00:00:00+00:00' WHERE state = 'pending'",
            [],
        )
        .expect("rewind due");
        let handler = |_: &mut rusqlite::Connection,
                       _: &lam_storage::Job|
         -> Result<String, lam_core::LamError> {
            Err(lam_core::LamError::validation(
                "E_REMOTE_DEFERRED",
                "remote extraction deferred to maintain".to_string(),
            ))
        };
        let outcome = maintain(&mut conn, "hook-startup", 5000, None, &handler);
        assert_eq!(outcome.deferred, 1);
        assert_eq!(outcome.failed, 0);
        assert_eq!(outcome.succeeded, 0);
        let counts = lam_storage::counts_by_state(&conn).expect("counts");
        let pending = counts
            .iter()
            .find(|(state, _)| state == "pending")
            .map(|(_, count)| *count)
            .unwrap_or(0);
        assert_eq!(pending, 1);
        let attempts: i64 = conn
            .query_row(
                "SELECT attempts FROM maintenance_jobs WHERE kind = 'fact_extract'",
                [],
                |row| row.get(0),
            )
            .expect("attempts");
        assert_eq!(attempts, 0);
        let outcome = maintain(&mut conn, "cli-maintain", 5000, None, &|_, _| {
            Ok("done".to_string())
        });
        assert_eq!(outcome.succeeded, 1);
    }
}
