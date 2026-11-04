use chrono::DateTime;
use chrono::Utc;

pub const TRANSFER_SCHEMA_VERSION: u32 = 1;

type RevisionRow = (
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    Option<String>,
);

type ProjectRow = (String, String, String, String, String);

#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub projects: usize,
    pub memories: usize,
    pub revisions: usize,
    pub sources: usize,
    pub facts: usize,
    pub skipped: usize,
    pub redacted: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Record {
    schema_version: u32,
    #[serde(rename = "type")]
    kind: String,
    #[serde(flatten)]
    body: serde_json::Value,
}

fn record(kind: &str, body: serde_json::Value) -> String {
    serde_json::to_string(&Record {
        schema_version: TRANSFER_SCHEMA_VERSION,
        kind: kind.to_string(),
        body,
    })
    .unwrap_or_default()
}

fn scrub_content(text: &str) -> (String, bool) {
    if lam_core::secrets::find_secret_kind(text).is_some() {
        ("[REDACTED:export]".to_string(), true)
    } else {
        (text.to_string(), false)
    }
}

pub fn export_jsonl(
    conn: &rusqlite::Connection,
    project: Option<&lam_core::ProjectId>,
) -> Result<(Vec<String>, usize), lam_core::LamError> {
    let mut lines = Vec::new();
    let mut redacted = 0;
    let projects: Vec<ProjectRow> = {
        let mut sql =
            "SELECT id, name, canonical_root, identity_hash, created_at FROM projects".to_string();
        if project.is_some() {
            sql.push_str(" WHERE id = ?1");
        }
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let rows: Result<Vec<_>, _> = if let Some(id) = project {
            stmt.query_map(rusqlite::params![id.to_string_value()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?
            .collect()
        } else {
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?
            .collect()
        };
        rows.map_err(|e| lam_core::LamError::storage(e.to_string()))?
    };
    let project_ids: Vec<String> = projects.iter().map(|row| row.0.clone()).collect();
    for (id, name, root, hash, created) in &projects {
        lines.push(record(
            "project",
            serde_json::json!({
                "id": id, "name": name, "canonical_root": root,
                "identity_hash": hash, "created_at": created,
            }),
        ));
    }
    for project_id in &project_ids {
        let mut stmt = conn
            .prepare(
                "SELECT id, project_id, scope, kind, current_revision, created_at FROM memories WHERE project_id = ?1 ORDER BY rowid ASC",
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let memory_ids: Vec<String> = stmt
            .query_map(rusqlite::params![project_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?
            .into_iter()
            .map(|row| {
                lines.push(record(
                    "memory",
                    serde_json::json!({
                        "id": row.0, "project_id": row.1, "scope": row.2,
                        "kind": row.3, "current_revision": row.4, "created_at": row.5,
                    }),
                ));
                row.0
            })
            .collect();
        for memory_id in &memory_ids {
            let mut rev_stmt = conn
                .prepare(
                    "SELECT id, memory_id, content, status, version, recorded_at, actor, reason
                     FROM memory_revisions WHERE memory_id = ?1 ORDER BY version ASC",
                )
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            let revisions: Vec<RevisionRow> = rev_stmt
                .query_map(rusqlite::params![memory_id], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                })
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            for (id, mid, content, status, version, recorded, actor, reason) in &revisions {
                let (safe_content, hit) = scrub_content(content);
                redacted += usize::from(hit);
                lines.push(record(
                    "revision",
                    serde_json::json!({
                        "id": id, "memory_id": mid, "content": safe_content,
                        "status": status, "version": version, "recorded_at": recorded,
                        "actor": actor, "reason": reason,
                    }),
                ));
                let mut src_stmt = conn
                    .prepare(
                        "SELECT id, source_kind, source_ref, line_start, line_end, commit_sha, content_hash
                         FROM memory_sources WHERE revision_id = ?1 ORDER BY rowid ASC",
                    )
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                let sources = src_stmt
                    .query_map(rusqlite::params![id], |row| {
                        Ok(serde_json::json!({
                            "id": row.get::<_, String>(0)?,
                            "revision_id": id,
                            "source_kind": row.get::<_, String>(1)?,
                            "source_ref": row.get::<_, String>(2)?,
                            "line_start": row.get::<_, Option<i64>>(3)?,
                            "line_end": row.get::<_, Option<i64>>(4)?,
                            "commit_sha": row.get::<_, Option<String>>(5)?,
                            "content_hash": row.get::<_, Option<String>>(6)?,
                        }))
                    })
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                for source in &sources {
                    lines.push(record("source", source.clone()));
                }
                let mut fact_stmt = conn
                    .prepare(
                        "SELECT id, memory_revision_id, subject, predicate, value_json, valid_from,
                                valid_until, recorded_at, retracted_at, confidence
                         FROM fact_assertions WHERE memory_revision_id = ?1 ORDER BY rowid ASC",
                    )
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                let facts = fact_stmt
                    .query_map(rusqlite::params![id], |row| {
                        Ok(serde_json::json!({
                            "id": row.get::<_, String>(0)?,
                            "memory_revision_id": row.get::<_, String>(1)?,
                            "subject": row.get::<_, String>(2)?,
                            "predicate": row.get::<_, String>(3)?,
                            "value_json": row.get::<_, String>(4)?,
                            "valid_from": row.get::<_, String>(5)?,
                            "valid_until": row.get::<_, Option<String>>(6)?,
                            "recorded_at": row.get::<_, String>(7)?,
                            "retracted_at": row.get::<_, Option<String>>(8)?,
                            "confidence": row.get::<_, f64>(9)?,
                        }))
                    })
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                for fact in &facts {
                    lines.push(record("fact", fact.clone()));
                }
            }
        }
    }
    Ok((lines, redacted))
}

pub fn import_jsonl(
    conn: &mut rusqlite::Connection,
    lines: &[String],
    _now: DateTime<Utc>,
) -> Result<ImportReport, lam_core::LamError> {
    let mut report = ImportReport::default();
    let tx = conn
        .transaction()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Record = serde_json::from_str(trimmed).map_err(|e| {
            lam_core::LamError::validation("E_IMPORT_INVALID", format!("bad record: {e}"))
        })?;
        if parsed.schema_version != TRANSFER_SCHEMA_VERSION {
            return Err(lam_core::LamError::validation(
                "E_IMPORT_VERSION",
                format!("unsupported schema_version {}", parsed.schema_version),
            ));
        }
        let get = |name: &str| -> Result<&serde_json::Value, lam_core::LamError> {
            parsed.body.get(name).ok_or_else(|| {
                lam_core::LamError::validation(
                    "E_IMPORT_INVALID",
                    format!("record {} misses {name}", parsed.kind),
                )
            })
        };
        let text = |name: &str| -> Result<String, lam_core::LamError> {
            get(name)?
                .as_str()
                .map(|value| value.to_string())
                .ok_or_else(|| {
                    lam_core::LamError::validation(
                        "E_IMPORT_INVALID",
                        format!("record {} field {name} must be a string", parsed.kind),
                    )
                })
        };
        match parsed.kind.as_str() {
            "project" => {
                let changed = tx
                    .execute(
                        "INSERT OR IGNORE INTO projects (id, name, canonical_root, identity_hash, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![
                            text("id")?,
                            text("name")?,
                            text("canonical_root")?,
                            text("identity_hash")?,
                            text("created_at")?,
                        ],
                    )
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                report.projects += changed;
                report.skipped += 1 - changed;
            }
            "memory" => {
                let current = parsed
                    .body
                    .get("current_revision")
                    .and_then(|value| value.as_str().map(|text| text.to_string()));
                let changed = tx
                    .execute(
                        "INSERT OR IGNORE INTO memories (id, project_id, scope, kind, current_revision, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            text("id")?,
                            text("project_id")?,
                            text("scope")?,
                            text("kind")?,
                            current,
                            text("created_at")?,
                        ],
                    )
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                report.memories += changed;
                report.skipped += 1 - changed;
            }
            "revision" => {
                let (safe_content, hit) = scrub_content(&text("content")?);
                report.redacted += usize::from(hit);
                let version: i64 = parsed
                    .body
                    .get("version")
                    .and_then(|value| value.as_i64())
                    .ok_or_else(|| {
                        lam_core::LamError::validation(
                            "E_IMPORT_INVALID",
                            "record revision field version must be an integer".to_string(),
                        )
                    })?;
                let changed = tx
                    .execute(
                        "INSERT OR IGNORE INTO memory_revisions (id, memory_id, content, status, version, recorded_at, actor, reason)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            text("id")?,
                            text("memory_id")?,
                            safe_content,
                            text("status")?,
                            version,
                            text("recorded_at")?,
                            text("actor")?,
                            parsed.body.get("reason").and_then(|value| value.as_str()),
                        ],
                    )
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                report.revisions += changed;
                report.skipped += 1 - changed;
            }
            "source" => {
                let line_start: Option<i64> = parsed
                    .body
                    .get("line_start")
                    .and_then(|value| value.as_i64());
                let line_end: Option<i64> =
                    parsed.body.get("line_end").and_then(|value| value.as_i64());
                let changed = tx
                    .execute(
                        "INSERT OR IGNORE INTO memory_sources (id, revision_id, source_kind, source_ref, line_start, line_end, commit_sha, content_hash)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            text("id")?,
                            text("revision_id")?,
                            text("source_kind")?,
                            text("source_ref")?,
                            line_start,
                            line_end,
                            parsed.body.get("commit_sha").and_then(|value| value.as_str()),
                            parsed.body.get("content_hash").and_then(|value| value.as_str()),
                        ],
                    )
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                report.sources += changed;
                report.skipped += 1 - changed;
            }
            "fact" => {
                let confidence: f64 = parsed
                    .body
                    .get("confidence")
                    .and_then(|value| value.as_f64())
                    .ok_or_else(|| {
                        lam_core::LamError::validation(
                            "E_IMPORT_INVALID",
                            "record fact field confidence must be a number".to_string(),
                        )
                    })?;
                let changed = tx
                    .execute(
                        "INSERT OR IGNORE INTO fact_assertions (id, memory_revision_id, subject, predicate, value_json, valid_from,
                                valid_until, recorded_at, retracted_at, confidence)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        rusqlite::params![
                            text("id")?,
                            text("memory_revision_id")?,
                            text("subject")?,
                            text("predicate")?,
                            text("value_json")?,
                            text("valid_from")?,
                            parsed.body.get("valid_until").and_then(|value| value.as_str()),
                            text("recorded_at")?,
                            parsed.body.get("retracted_at").and_then(|value| value.as_str()),
                            confidence,
                        ],
                    )
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                report.facts += changed;
                report.skipped += 1 - changed;
            }
            other => {
                return Err(lam_core::LamError::validation(
                    "E_IMPORT_INVALID",
                    format!("unknown record type {other}"),
                ));
            }
        }
    }
    tx.commit()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    #[test]
    fn export_import_round_trips() {
        let mut source = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let project = crate::projects::resolve_or_create(
            &mut source,
            dir.path(),
            Some("round-trip"),
            chrono::Utc::now(),
        )
        .expect("project")
        .id;
        let (memory_id, revision_id) = crate::save_memory(
            &mut source,
            crate::SaveInput {
                project_id: project,
                scope: lam_core::Scope::Project,
                kind: lam_core::MemoryKind::Lesson,
                content: "Cache uses redis for sessions".to_string(),
                status: lam_core::MemoryStatus::Verified,
                actor: "cli".to_string(),
                reason: Some("test".to_string()),
                sources: vec![lam_core::SourceEvidence {
                    source_kind: lam_core::SourceKind::Episode,
                    source_ref: "episode:1".to_string(),
                    line_start: None,
                    line_end: None,
                    commit_sha: None,
                    content_hash: None,
                }],
                idempotency_key: None,
            },
            chrono::Utc::now(),
        )
        .expect("save");
        let (lines, redacted) = export_jsonl(&source, Some(&project)).expect("export");
        assert_eq!(redacted, 0);
        assert!(lines.iter().any(|line| line.contains("Cache uses redis")));
        let mut target = migrated_conn();
        let report = import_jsonl(&mut target, &lines, chrono::Utc::now()).expect("import");
        assert_eq!(report.memories, 1);
        assert_eq!(report.revisions, 1);
        assert_eq!(report.sources, 1);
        let detail = crate::get_memory(&target, &memory_id).expect("memory");
        assert_eq!(detail.revision.content, "Cache uses redis for sessions");
        assert_eq!(detail.revision.id, revision_id);
        assert_eq!(detail.sources.len(), 1);
        let again = import_jsonl(&mut target, &lines, chrono::Utc::now()).expect("reimport");
        assert_eq!(again.memories, 0);
        assert!(again.skipped > 0);
    }

    #[test]
    fn export_scrubs_secrets() {
        let mut source = migrated_conn();
        let dir = tempfile::tempdir().expect("tempdir");
        let project = crate::projects::resolve_or_create(
            &mut source,
            dir.path(),
            Some("scrub-proj"),
            chrono::Utc::now(),
        )
        .expect("project")
        .id;
        source
            .execute(
                "INSERT INTO memories (id, project_id, scope, kind, current_revision, created_at)
                 VALUES ('mem-1', ?1, 'project', 'note', 'rev-1', '2026-11-02T00:00:00Z')",
                rusqlite::params![project.to_string_value()],
            )
            .expect("memory");
        source
            .execute(
                "INSERT INTO memory_revisions (id, memory_id, content, status, version, recorded_at, actor, reason)
                 VALUES ('rev-1', 'mem-1', 'api_key=live-value-here', 'proposed', 1, '2026-11-02T00:00:00Z', 't', NULL)",
                [],
            )
            .expect("revision");
        let (lines, redacted) = export_jsonl(&source, Some(&project)).expect("export");
        assert_eq!(redacted, 1);
        assert!(lines.iter().all(|line| !line.contains("live-value-here")));
    }

    #[test]
    fn bad_schema_version_rejected() {
        let mut target = migrated_conn();
        let lines = vec!["{\"schema_version\":99,\"type\":\"project\",\"id\":\"x\"}".to_string()];
        assert!(import_jsonl(&mut target, &lines, chrono::Utc::now()).is_err());
    }
}
