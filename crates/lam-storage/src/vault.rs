use chrono::DateTime;
use chrono::Utc;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct VaultRoot {
    pub id: String,
    pub path: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub processed: usize,
    pub skipped: usize,
    pub stale: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct VaultHit {
    pub document_id: String,
    pub rel_path: String,
    pub title: String,
    pub snippet: String,
    pub chunk_id: String,
    pub line_start: u32,
    pub line_end: u32,
    pub rank: f64,
}

#[derive(Debug, Clone)]
pub struct BoundedRead {
    pub document_id: String,
    pub rel_path: String,
    pub title: String,
    pub total_lines: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct DocLink {
    pub other_id: String,
    pub other_path: String,
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DocLinks {
    pub outgoing: Vec<DocLink>,
    pub incoming: Vec<DocLink>,
}

fn doc_id_for(root_id: &str, rel_path: &str) -> String {
    blake3::hash(format!("lam-document:v1:{root_id}\n{rel_path}").as_bytes())
        .to_hex()
        .to_string()
}

fn chunk_id_for(doc_id: &str, ordinal: usize, chunk_hash: &str) -> String {
    blake3::hash(format!("lam-chunk:v1:{doc_id}\n{ordinal}\n{chunk_hash}").as_bytes())
        .to_hex()
        .to_string()
}

pub fn register_root(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    path: &std::path::Path,
    now: DateTime<Utc>,
) -> Result<String, lam_core::LamError> {
    let project = crate::projects::lookup_by_id(conn, project_id)?;
    let canonical = std::fs::canonicalize(path).map_err(|e| {
        lam_core::LamError::validation("E_VAULT_ROOT", format!("vault root unreadable: {e}"))
    })?;
    if !canonical.is_dir() {
        return Err(lam_core::LamError::validation(
            "E_VAULT_ROOT",
            "vault root is not a directory".to_string(),
        ));
    }
    if canonical != project.canonical_root && !canonical.starts_with(&project.canonical_root) {
        return Err(lam_core::LamError::validation(
            "E_VAULT_SCOPE",
            "vault root must equal the project directory or sit beneath it".to_string(),
        ));
    }
    let root_id = blake3::hash(
        format!(
            "lam-vault-root:v1:{}\n{}",
            project_id.to_string_value(),
            canonical.to_string_lossy()
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    conn.execute(
        "INSERT INTO vault_roots (id, project_id, path, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO NOTHING",
        rusqlite::params![
            root_id,
            project_id.to_string_value(),
            canonical.to_string_lossy(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(root_id)
}

pub fn list_roots(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
) -> Result<Vec<VaultRoot>, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, path, created_at FROM vault_roots WHERE project_id = ?1 ORDER BY path ASC",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![project_id.to_string_value()], |row| {
            Ok(VaultRoot {
                id: row.get(0)?,
                path: row.get(1)?,
                created_at: row.get(2)?,
            })
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| lam_core::LamError::storage(e.to_string()))?);
    }
    Ok(out)
}

pub fn unregister_root(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    path_or_id: &str,
) -> Result<bool, lam_core::LamError> {
    let roots = list_roots(conn, project_id)?;
    let trimmed = path_or_id.trim();
    let canonical_input = std::fs::canonicalize(trimmed)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| trimmed.to_string());
    let target = roots.iter().find(|root| {
        root.id == trimmed
            || root.path == trimmed
            || root.path == canonical_input
            || std::path::Path::new(&root.path)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(trimmed)
    });
    let root = match target {
        Some(found) => found.clone(),
        None => return Ok(false),
    };
    let doc_ids: Vec<String> = conn
        .prepare("SELECT id FROM documents WHERE project_id = ?1 AND root_id = ?2")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .query_map(
            rusqlite::params![project_id.to_string_value(), root.id],
            |row| row.get(0),
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for doc_id in doc_ids {
        delete_document_index(conn, &doc_id)?;
        conn.execute(
            "DELETE FROM documents WHERE id = ?1",
            rusqlite::params![doc_id],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    }
    conn.execute(
        "DELETE FROM vault_roots WHERE id = ?1",
        rusqlite::params![root.id],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(true)
}

fn delete_document_index(
    conn: &rusqlite::Connection,
    doc_id: &str,
) -> Result<(), lam_core::LamError> {
    let chunk_ids: Vec<String> = conn
        .prepare("SELECT id FROM document_chunks WHERE document_id = ?1")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .query_map(rusqlite::params![doc_id], |row| row.get(0))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for chunk_id in chunk_ids {
        conn.execute(
            "DELETE FROM vector_index WHERE owner_id = ?1",
            rusqlite::params![chunk_id],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    }
    conn.execute(
        "DELETE FROM document_chunks WHERE document_id = ?1",
        rusqlite::params![doc_id],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute(
        "DELETE FROM document_fts WHERE document_id = ?1",
        rusqlite::params![doc_id],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute(
        "DELETE FROM document_links WHERE source_document_id = ?1 OR target_document_id = ?1",
        rusqlite::params![doc_id],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(())
}

pub fn sync_project(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    policy: &lam_vault::ScanPolicy,
    now: DateTime<Utc>,
) -> Result<SyncReport, lam_core::LamError> {
    let mut report = SyncReport::default();
    let roots = list_roots(conn, project_id)?;
    let project_policy = crate::policies::get_policy(conn, project_id)?;
    let mut known_paths: Vec<String> = Vec::new();
    for root in &roots {
        let root_path = std::path::Path::new(&root.path);
        let scanned = match lam_vault::scan_root(root_path, policy) {
            Ok(files) => files,
            Err(e) => {
                push_warning(
                    &mut report,
                    format!("root {} skipped: {}", root.path, e.code()),
                );
                continue;
            }
        };
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for file in scanned {
            if project_policy.denied(&file.rel_path) {
                report.skipped += 1;
                push_warning(
                    &mut report,
                    format!("deny-listed path skipped: {}", file.rel_path),
                );
                continue;
            }
            seen.insert(file.rel_path.clone());
            known_paths.push(file.rel_path.clone());
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT id, hash FROM documents WHERE project_id = ?1 AND root_id = ?2 AND relative_path = ?3",
                    rusqlite::params![project_id.to_string_value(), root.id, file.rel_path],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            if let Some((doc_id, hash)) = existing {
                let chunks: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM document_chunks WHERE document_id = ?1",
                        rusqlite::params![doc_id],
                        |row| row.get(0),
                    )
                    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                if hash == file.hash && chunks > 0 {
                    report.skipped += 1;
                    continue;
                }
                index_file(conn, project_id, &root.id, &doc_id, &file, now)?;
                report.processed += 1;
            } else {
                let doc_id = doc_id_for(&root.id, &file.rel_path);
                index_file(conn, project_id, &root.id, &doc_id, &file, now)?;
                report.processed += 1;
            }
        }
        let stored: Vec<(String, String)> = conn
            .prepare(
                "SELECT id, relative_path FROM documents WHERE project_id = ?1 AND root_id = ?2",
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?
            .query_map(
                rusqlite::params![project_id.to_string_value(), root.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?
            .collect::<Result<Vec<(String, String)>, _>>()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        for (doc_id, rel_path) in stored {
            if !seen.contains(&rel_path) {
                delete_document_index(conn, &doc_id)?;
                conn.execute(
                    "UPDATE documents SET status = 'stale', hash = '' WHERE id = ?1",
                    rusqlite::params![doc_id],
                )
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                report.stale += 1;
            }
        }
    }
    rebuild_links(conn, project_id, &mut report)?;
    crate::db::harden_current_db(conn);
    Ok(report)
}

fn index_file(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    root_id: &str,
    doc_id: &str,
    file: &lam_vault::ScannedFile,
    now: DateTime<Utc>,
) -> Result<(), lam_core::LamError> {
    let bytes = std::fs::read(&file.abs_path).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let stem = std::path::Path::new(&file.rel_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("document");
    let parsed = lam_vault::parse_bytes(file.kind, &bytes, stem)?;
    let chunks = match file.kind {
        lam_vault::parse::FileKind::Markdown => lam_vault::chunk_markdown(&parsed.text),
        _ => lam_vault::chunk_text(&parsed.text),
    };
    let title = parsed.title.unwrap_or_else(|| file.rel_path.clone());
    delete_document_index(conn, doc_id)?;
    conn.execute(
        "INSERT INTO documents (id, project_id, root_id, relative_path, hash, title, indexed_at, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'ok')
         ON CONFLICT(id) DO UPDATE SET hash = excluded.hash, title = excluded.title,
         indexed_at = excluded.indexed_at, status = 'ok'",
        rusqlite::params![
            doc_id,
            project_id.to_string_value(),
            root_id,
            file.rel_path,
            file.hash,
            title,
            now.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for chunk in chunks {
        let chunk_id = chunk_id_for(doc_id, chunk.ordinal, &chunk.hash);
        conn.execute(
            "INSERT INTO document_chunks (id, document_id, ordinal, content, line_start, line_end, hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET content = excluded.content, line_start = excluded.line_start,
             line_end = excluded.line_end, hash = excluded.hash",
            rusqlite::params![
                chunk_id,
                doc_id,
                chunk.ordinal as i64,
                chunk.content,
                chunk.line_start as i64,
                chunk.line_end as i64,
                chunk.hash,
            ],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        conn.execute(
            "INSERT INTO document_fts (document_id, chunk_id, content, title, relative_path)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![doc_id, chunk_id, chunk.content, title, file.rel_path],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    }
    crate::db::harden_current_db(conn);
    Ok(())
}

fn push_warning(report: &mut SyncReport, message: String) {
    if report.warnings.len() < 50 && !report.warnings.contains(&message) {
        report.warnings.push(message);
    }
}

fn rebuild_links(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    report: &mut SyncReport,
) -> Result<(), lam_core::LamError> {
    let docs: Vec<(String, String)> = conn
        .prepare("SELECT id, relative_path FROM documents WHERE project_id = ?1 AND status = 'ok'")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .query_map(rusqlite::params![project_id.to_string_value()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .collect::<Result<Vec<(String, String)>, _>>()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let known: Vec<String> = docs.iter().map(|(_, rel)| rel.clone()).collect();
    let by_id: std::collections::HashMap<String, String> = docs
        .iter()
        .map(|(id, rel)| (rel.clone(), id.clone()))
        .collect();
    conn.execute(
        "DELETE FROM document_links WHERE source_document_id IN
         (SELECT id FROM documents WHERE project_id = ?1)",
        rusqlite::params![project_id.to_string_value()],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for (doc_id, rel_path) in &docs {
        let text = document_text(conn, doc_id)?;
        for link in lam_vault::extract_wikilinks(&text) {
            match lam_vault::resolve_link(&known, &link.target) {
                lam_vault::LinkTarget::Exact(path) | lam_vault::LinkTarget::Unique(path) => {
                    if let Some(target_id) = by_id.get(&path) {
                        if target_id != doc_id {
                            conn.execute(
                                "INSERT OR IGNORE INTO document_links
                                 (source_document_id, target_document_id, anchor, link_type)
                                 VALUES (?1, ?2, ?3, 'wikilink')",
                                rusqlite::params![doc_id, target_id, link.anchor],
                            )
                            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                        }
                    }
                }
                lam_vault::LinkTarget::Ambiguous(options) => {
                    push_warning(
                        report,
                        format!(
                            "{rel_path}: ambiguous link [[{}]] matches {}",
                            link.target,
                            options.len()
                        ),
                    );
                }
                lam_vault::LinkTarget::Missing => {
                    push_warning(
                        report,
                        format!("{rel_path}: unresolved link [[{}]]", link.target),
                    );
                }
            }
        }
    }
    Ok(())
}

fn document_text(conn: &rusqlite::Connection, doc_id: &str) -> Result<String, lam_core::LamError> {
    let mut stmt = conn
        .prepare("SELECT content FROM document_chunks WHERE document_id = ?1 ORDER BY ordinal ASC")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![doc_id], |row| row.get::<_, String>(0))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = String::new();
    for row in rows {
        let content = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&content);
    }
    Ok(out)
}

fn fallback_query(raw: &str) -> Option<String> {
    let tokens: Vec<String> = raw
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(10)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" OR "))
    }
}

pub fn search_documents(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    query: &str,
    limit: usize,
) -> Result<Vec<VaultHit>, lam_core::LamError> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_QUERY_INVALID",
            "query must not be empty".to_string(),
        ));
    }
    if limit == 0 || limit > 100 {
        return Err(lam_core::LamError::validation(
            "E_LIMIT_INVALID",
            "limit must be within 1 and 100".to_string(),
        ));
    }
    match search_documents_fts(conn, project_id, trimmed, limit as i64) {
        Ok(hits) if !hits.is_empty() => Ok(hits),
        Ok(_) => match fallback_query(trimmed) {
            Some(fallback) if fallback != trimmed => {
                search_documents_fts(conn, project_id, &fallback, limit as i64)
            }
            _ => Ok(Vec::new()),
        },
        Err(e) if crate::db::is_retryable_message(&e.to_string()) => Err(e),
        Err(original) => {
            if let Some(fallback) = fallback_query(trimmed) {
                search_documents_fts(conn, project_id, &fallback, limit as i64)
                    .map_err(|_| original)
            } else {
                Err(original)
            }
        }
    }
}

fn search_documents_fts(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    fts_query: &str,
    limit: i64,
) -> Result<Vec<VaultHit>, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT f.document_id, d.relative_path, d.title, f.chunk_id,
                    snippet(document_fts, 2, '<<', '>>', '...', 24),
                    bm25(document_fts)
             FROM document_fts f
             JOIN documents d ON d.id = f.document_id
             WHERE f.document_fts MATCH ?1 AND d.project_id = ?2 AND d.status = 'ok'
             ORDER BY bm25(document_fts)
             LIMIT ?3",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(
            rusqlite::params![fts_query, project_id.to_string_value(), limit * 3],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, f64>(5)?,
                ))
            },
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out: Vec<VaultHit> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows {
        let tuple = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if !seen.insert(tuple.3.clone()) {
            continue;
        }
        let lines: Option<(i64, i64)> = conn
            .query_row(
                "SELECT line_start, line_end FROM document_chunks WHERE id = ?1",
                rusqlite::params![tuple.3],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if out.len() >= limit as usize {
            break;
        }
        out.push(VaultHit {
            document_id: tuple.0,
            rel_path: tuple.1.clone(),
            title: tuple.2.unwrap_or_else(|| tuple.1.clone()),
            snippet: tuple.4.unwrap_or_default(),
            chunk_id: tuple.3,
            line_start: lines.map(|(start, _)| start as u32).unwrap_or(1),
            line_end: lines.map(|(_, end)| end as u32).unwrap_or(1),
            rank: tuple.5,
        });
    }
    Ok(out)
}

fn resolve_document(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    doc_ref: &str,
) -> Result<(String, String), lam_core::LamError> {
    let trimmed = doc_ref.trim();
    if trimmed.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_DOC_REF_INVALID",
            "document reference must not be empty".to_string(),
        ));
    }
    let by_id: Option<(String, String)> = conn
        .query_row(
            "SELECT id, relative_path FROM documents WHERE id = ?1 AND project_id = ?2",
            rusqlite::params![trimmed, project_id.to_string_value()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if let Some(found) = by_id {
        return Ok(found);
    }
    let by_path: Option<(String, String)> = conn
        .query_row(
            "SELECT id, relative_path FROM documents WHERE relative_path = ?1 AND project_id = ?2",
            rusqlite::params![trimmed, project_id.to_string_value()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    by_path.ok_or_else(|| {
        lam_core::LamError::validation("E_DOC_NOT_FOUND", format!("no vault document {trimmed}"))
    })
}

pub fn read_document(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    doc_ref: &str,
    max_bytes: usize,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<BoundedRead, lam_core::LamError> {
    if max_bytes == 0 || max_bytes > 1_048_576 {
        return Err(lam_core::LamError::validation(
            "E_READ_BOUND_INVALID",
            "max bytes must be within 1 and 1048576".to_string(),
        ));
    }
    let (doc_id, rel_path) = resolve_document(conn, project_id, doc_ref)?;
    let roots = list_roots(conn, project_id)?;
    if roots.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_DOC_NOT_FOUND",
            "document root is gone; sync the vault".to_string(),
        ));
    }
    let root_id: Option<String> = conn
        .query_row(
            "SELECT root_id FROM documents WHERE id = ?1",
            rusqlite::params![doc_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let root_id = match root_id {
        Some(id) => id,
        None => {
            return Err(lam_core::LamError::validation(
                "E_DOC_NOT_FOUND",
                "document root is gone; sync the vault".to_string(),
            ));
        }
    };
    let root_path: PathBuf = roots
        .iter()
        .find(|root| root.id == root_id)
        .map(|root| PathBuf::from(&root.path))
        .ok_or_else(|| {
            lam_core::LamError::validation(
                "E_DOC_NOT_FOUND",
                "document root is gone; sync the vault".to_string(),
            )
        })?;
    let abs_path = root_path.join(&rel_path);
    let canonical = std::fs::canonicalize(&abs_path).map_err(|_| {
        lam_core::LamError::validation(
            "E_DOC_GONE",
            "document file is gone; sync the vault".to_string(),
        )
    })?;
    let canonical_root =
        std::fs::canonicalize(&root_path).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(lam_core::LamError::validation(
            "E_VAULT_SCOPE",
            "document escapes the vault root".to_string(),
        ));
    }
    let bytes = std::fs::read(&canonical).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let text = String::from_utf8(bytes).map_err(|_| {
        lam_core::LamError::validation("E_VAULT_ENCODING", "file is not UTF-8 text".to_string())
    })?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let from = start_line.unwrap_or(1).max(1).min(total.max(1));
    let mut until = end_line.unwrap_or(total.max(1));
    if until < from {
        until = from;
    }
    if until > total {
        until = total;
    }
    let mut selected = if total == 0 {
        String::new()
    } else {
        lines[from - 1..until].join("\n")
    };
    let mut truncated = false;
    if selected.len() > max_bytes {
        let mut cutoff = max_bytes;
        while cutoff > 0 && !selected.is_char_boundary(cutoff) {
            cutoff -= 1;
        }
        selected.truncate(cutoff);
        truncated = true;
    }
    let title: Option<String> = conn
        .query_row(
            "SELECT title FROM documents WHERE id = ?1",
            rusqlite::params![doc_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(BoundedRead {
        document_id: doc_id,
        rel_path,
        title: title.unwrap_or_default(),
        total_lines: total,
        start_line: from,
        end_line: until,
        text: selected,
        truncated,
    })
}

pub fn links_of(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    doc_ref: &str,
) -> Result<DocLinks, lam_core::LamError> {
    let (doc_id, _) = resolve_document(conn, project_id, doc_ref)?;
    let mut outgoing = Vec::new();
    let mut incoming = Vec::new();
    let mut out_stmt = conn
        .prepare(
            "SELECT l.target_document_id, d.relative_path, l.anchor
             FROM document_links l JOIN documents d ON d.id = l.target_document_id
             WHERE l.source_document_id = ?1 AND l.link_type = 'wikilink'
             ORDER BY d.relative_path ASC",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let out_rows = out_stmt
        .query_map(rusqlite::params![doc_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for row in out_rows {
        let (id, path, anchor) = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        outgoing.push(DocLink {
            other_id: id,
            other_path: path,
            anchor,
        });
    }
    let mut in_stmt = conn
        .prepare(
            "SELECT l.source_document_id, d.relative_path, l.anchor
             FROM document_links l JOIN documents d ON d.id = l.source_document_id
             WHERE l.target_document_id = ?1 AND l.link_type = 'wikilink'
             ORDER BY d.relative_path ASC",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let in_rows = in_stmt
        .query_map(rusqlite::params![doc_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for row in in_rows {
        let (id, path, anchor) = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        incoming.push(DocLink {
            other_id: id,
            other_path: path,
            anchor,
        });
    }
    Ok(DocLinks { outgoing, incoming })
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
    use std::io::Write;

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

    fn project_at(conn: &mut rusqlite::Connection, path: &std::path::Path) -> lam_core::ProjectId {
        crate::projects::resolve_or_create(conn, path, Some("vault-proj"), at())
            .expect("project")
            .id
    }

    fn write_tree(root: &std::path::Path) {
        let mut guide = std::fs::File::create(root.join("guide.md")).expect("guide");
        guide
            .write_all(b"# Guide\n\nSee [[docs/adr.md]] for decisions.\n\nBody text here.\n")
            .expect("write");
        std::fs::create_dir_all(root.join("docs")).expect("docs");
        let mut adr = std::fs::File::create(root.join("docs").join("adr.md")).expect("adr");
        adr.write_all(b"# ADR\n\nDecision record.\n")
            .expect("write");
    }

    fn policy() -> lam_vault::ScanPolicy {
        lam_vault::ScanPolicy::new(1_048_576, false).expect("policy")
    }

    #[test]
    fn sync_counts_processed_and_skipped() {
        let mut conn = migrated_conn();
        let work = tempfile::tempdir().expect("work");
        let project_id = project_at(&mut conn, work.path());
        write_tree(work.path());
        register_root(&conn, &project_id, work.path(), at()).expect("register");
        let first = sync_project(&conn, &project_id, &policy(), at()).expect("sync");
        assert_eq!(first.processed, 2);
        assert_eq!(first.skipped, 0);
        let second = sync_project(&conn, &project_id, &policy(), at()).expect("resync");
        assert_eq!(second.processed, 0);
        assert_eq!(second.skipped, 2);
    }

    #[test]
    fn changed_file_only_reprocesses() {
        let mut conn = migrated_conn();
        let work = tempfile::tempdir().expect("work");
        let project_id = project_at(&mut conn, work.path());
        write_tree(work.path());
        register_root(&conn, &project_id, work.path(), at()).expect("register");
        sync_project(&conn, &project_id, &policy(), at()).expect("sync");
        let mut guide = std::fs::OpenOptions::new()
            .append(true)
            .open(work.path().join("guide.md"))
            .expect("open");
        guide
            .write_all(b"\nExtra paragraph added later.\n")
            .expect("append");
        let report = sync_project(&conn, &project_id, &policy(), at()).expect("resync");
        assert_eq!(report.processed, 1);
        assert_eq!(report.skipped, 1);
    }

    #[test]
    fn backlinks_resolve() {
        let mut conn = migrated_conn();
        let work = tempfile::tempdir().expect("work");
        let project_id = project_at(&mut conn, work.path());
        write_tree(work.path());
        register_root(&conn, &project_id, work.path(), at()).expect("register");
        let report = sync_project(&conn, &project_id, &policy(), at()).expect("sync");
        assert!(report.warnings.is_empty());
        let links = links_of(&conn, &project_id, "docs/adr.md").expect("links");
        assert_eq!(links.incoming.len(), 1);
        assert_eq!(links.incoming[0].other_path, "guide.md");
        let guide = links_of(&conn, &project_id, "guide.md").expect("guide links");
        assert_eq!(guide.outgoing.len(), 1);
        assert_eq!(guide.outgoing[0].other_path, "docs/adr.md");
    }

    #[test]
    fn deleted_files_marked_stale() {
        let mut conn = migrated_conn();
        let work = tempfile::tempdir().expect("work");
        let project_id = project_at(&mut conn, work.path());
        write_tree(work.path());
        register_root(&conn, &project_id, work.path(), at()).expect("register");
        sync_project(&conn, &project_id, &policy(), at()).expect("sync");
        std::fs::remove_file(work.path().join("docs").join("adr.md")).expect("remove");
        let report = sync_project(&conn, &project_id, &policy(), at()).expect("resync");
        assert_eq!(report.stale, 1);
        let hits = search_documents(&conn, &project_id, "Decision record", 5).expect("search");
        assert!(hits.iter().all(|hit| hit.rel_path != "docs/adr.md"));
    }

    #[test]
    fn search_returns_snippets_with_citations() {
        let mut conn = migrated_conn();
        let work = tempfile::tempdir().expect("work");
        let project_id = project_at(&mut conn, work.path());
        write_tree(work.path());
        register_root(&conn, &project_id, work.path(), at()).expect("register");
        sync_project(&conn, &project_id, &policy(), at()).expect("sync");
        let hits = search_documents(&conn, &project_id, "decisions", 5).expect("search");
        assert!(!hits.is_empty());
        assert!(hits[0].line_start >= 1);
        assert!(!hits[0].snippet.is_empty());
    }

    #[test]
    fn read_is_bounded_by_lines_and_bytes() {
        let mut conn = migrated_conn();
        let work = tempfile::tempdir().expect("work");
        let project_id = project_at(&mut conn, work.path());
        write_tree(work.path());
        register_root(&conn, &project_id, work.path(), at()).expect("register");
        sync_project(&conn, &project_id, &policy(), at()).expect("sync");
        let bounded =
            read_document(&conn, &project_id, "guide.md", 50, Some(1), Some(2)).expect("read");
        assert_eq!(bounded.start_line, 1);
        assert_eq!(bounded.end_line, 2);
        assert!(bounded.total_lines >= 3);
        let tiny = read_document(&conn, &project_id, "guide.md", 10, None, None).expect("read");
        assert!(tiny.truncated);
    }

    #[test]
    fn unregister_removes_index_not_sources() {
        let mut conn = migrated_conn();
        let work = tempfile::tempdir().expect("work");
        let project_id = project_at(&mut conn, work.path());
        write_tree(work.path());
        register_root(&conn, &project_id, work.path(), at()).expect("register");
        sync_project(&conn, &project_id, &policy(), at()).expect("sync");
        assert!(
            unregister_root(&conn, &project_id, work.path().to_string_lossy().as_ref())
                .expect("unregister")
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE project_id = ?1",
                rusqlite::params![project_id.to_string_value()],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(count, 0);
        assert!(work.path().join("guide.md").exists());
        assert!(!unregister_root(&conn, &project_id, "missing").expect("missing"));
    }

    #[test]
    fn root_outside_project_rejected() {
        let mut conn = migrated_conn();
        let home = tempfile::tempdir().expect("home");
        let project_id = project_at(&mut conn, home.path());
        let outside = tempfile::tempdir().expect("outside");
        let result = register_root(&conn, &project_id, outside.path(), at());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "E_VAULT_SCOPE");
    }
}
