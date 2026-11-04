#[derive(Debug, Clone)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

fn checksum(sql: &str) -> String {
    blake3::hash(sql.as_bytes()).to_hex().to_string()
}

pub static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../migrations/001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "fact_env_and_policy",
        sql: include_str!("../migrations/002_fact_env_and_policy.sql"),
    },
    Migration {
        version: 3,
        name: "vault_episode_vector",
        sql: include_str!("../migrations/003_vault_episode_vector.sql"),
    },
    Migration {
        version: 4,
        name: "dedup_suggestions",
        sql: include_str!("../migrations/004_dedup_suggestions.sql"),
    },
    Migration {
        version: 5,
        name: "remote_calls",
        sql: include_str!("../migrations/005_remote_calls.sql"),
    },
    Migration {
        version: 6,
        name: "project_policies",
        sql: include_str!("../migrations/006_project_policies.sql"),
    },
];

fn is_busy_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("database is locked")
        || lower.contains("database table is locked")
        || lower.contains("database is busy")
        || lower.contains("busy")
}

fn applied_checksum(
    conn: &rusqlite::Connection,
    version: u32,
) -> Result<Option<String>, lam_core::LamError> {
    conn.query_row(
        "SELECT checksum FROM schema_migrations WHERE version = ?1",
        rusqlite::params![version],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| lam_core::LamError::storage(e.to_string()))
}

pub fn apply_migrations(conn: &mut rusqlite::Connection) -> Result<Vec<u32>, lam_core::LamError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            checksum TEXT NOT NULL,
            applied_at TEXT NOT NULL
        ) STRICT;",
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut applied: Vec<u32> = Vec::new();
    for migration in MIGRATIONS {
        let expected = checksum(migration.sql);
        let mut attempt = 0;
        loop {
            attempt += 1;
            if let Some(found) = applied_checksum(conn, migration.version)? {
                if found != expected {
                    return Err(lam_core::LamError::storage(format!(
                        "migration {} checksum mismatch",
                        migration.version
                    )));
                }
                break;
            }
            match try_apply_one(conn, migration, &expected) {
                Ok(true) => {
                    applied.push(migration.version);
                    break;
                }
                Ok(false) => {
                    break;
                }
                Err(e) if is_busy_error(&e.to_string()) && attempt < 30 => {
                    std::thread::sleep(std::time::Duration::from_millis(10 * attempt as u64));
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(applied)
}

fn try_apply_one(
    conn: &mut rusqlite::Connection,
    migration: &Migration,
    expected: &str,
) -> Result<bool, lam_core::LamError> {
    let tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => return Err(lam_core::LamError::storage(e.to_string())),
    };
    let present: Option<String> = tx
        .query_row(
            "SELECT checksum FROM schema_migrations WHERE version = ?1",
            rusqlite::params![migration.version],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if let Some(found) = present {
        if found != expected {
            return Err(lam_core::LamError::storage(format!(
                "migration {} checksum mismatch",
                migration.version
            )));
        }
        return Ok(false);
    }
    if let Err(e) = tx.execute_batch(migration.sql) {
        let message = e.to_string();
        drop(tx);
        return recheck_after_conflict(conn, migration, expected, message);
    }
    let now = chrono::Utc::now().to_rfc3339();
    let insert = tx.execute(
        "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![migration.version, migration.name, expected, now],
    );
    if let Err(e) = insert {
        let message = e.to_string();
        drop(tx);
        return recheck_after_conflict(conn, migration, expected, message);
    }
    if let Err(e) = tx.commit() {
        let message = e.to_string();
        if is_busy_error(&message) {
            return Err(lam_core::LamError::storage(message));
        }
        return recheck_after_conflict(conn, migration, expected, message);
    }
    Ok(true)
}

fn recheck_after_conflict(
    conn: &rusqlite::Connection,
    migration: &Migration,
    expected: &str,
    original: String,
) -> Result<bool, lam_core::LamError> {
    match applied_checksum(conn, migration.version)? {
        Some(found) if found == expected => Ok(false),
        Some(_) => Err(lam_core::LamError::storage(format!(
            "migration {} checksum mismatch",
            migration.version
        ))),
        None => Err(lam_core::LamError::storage(original)),
    }
}

pub fn applied_versions(conn: &rusqlite::Connection) -> Result<Vec<u32>, lam_core::LamError> {
    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if table_exists == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT version FROM schema_migrations ORDER BY version ASC")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, u32>(0))
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
    #[test]
    fn migrations_apply_idempotently() {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory db");
        let first = apply_migrations(&mut conn).expect("first apply");
        assert_eq!(first, vec![1, 2, 3, 4, 5, 6]);
        let second = apply_migrations(&mut conn).expect("second apply");
        assert!(second.is_empty());
        let versions = applied_versions(&conn).expect("list versions");
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6]);
    }
}
