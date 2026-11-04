use chrono::DateTime;
use chrono::Utc;

use crate::types::GroupedHits;
use crate::types::HitKind;
use crate::types::UnifiedHit;

fn validated(query: &str, limit: usize) -> Result<(String, usize), lam_core::LamError> {
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
    Ok((trimmed.to_string(), limit))
}

pub fn multi_search(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    query: &str,
    per_source: usize,
    now: DateTime<Utc>,
) -> Result<GroupedHits, lam_core::LamError> {
    let (text, limit) = validated(query, per_source)?;
    let mut out = GroupedHits::default();
    for hit in lam_storage::search_memories(
        conn,
        lam_storage::SearchParams {
            project_id: *project_id,
            query: text.clone(),
            scope: None,
            include_history: false,
            limit,
        },
    )? {
        out.memories.push(UnifiedHit {
            id: hit.memory_id.to_string(),
            kind: HitKind::Memory,
            title: format!("{} {}", hit.kind.as_str(), hit.status.as_str()),
            snippet: hit.snippet.clone(),
            source_ref: if hit.source_ref.is_empty() {
                hit.memory_id.to_string()
            } else {
                hit.source_ref.clone()
            },
            rank: hit.rank,
            status: Some(hit.status.as_str().to_string()),
        });
    }
    for hit in lam_storage::search_episodes(conn, project_id, &text, limit, now)? {
        out.episodes.push(UnifiedHit {
            id: hit.episode_id.clone(),
            kind: HitKind::Episode,
            title: hit.abstract_text.clone(),
            snippet: hit.snippet.clone(),
            source_ref: hit.session_id.clone(),
            rank: hit.rank,
            status: None,
        });
    }
    for hit in lam_storage::search_documents(conn, project_id, &text, limit)? {
        out.vault.push(UnifiedHit {
            id: format!("{}:{}", hit.document_id, hit.chunk_id),
            kind: HitKind::Vault,
            title: hit.title.clone(),
            snippet: hit.snippet.clone(),
            source_ref: format!("{}:{}", hit.rel_path, hit.line_start),
            rank: hit.rank,
            status: None,
        });
    }
    let entities = lam_storage::search_entities(conn, project_id, &text, limit)?;
    for (position, entity) in entities.iter().enumerate() {
        out.graph.push(UnifiedHit {
            id: entity.id.to_string(),
            kind: HitKind::Graph,
            title: entity.canonical_name.clone(),
            snippet: entity.aliases.join(", "),
            source_ref: entity.canonical_name.clone(),
            rank: position as f64,
            status: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = lam_storage::open_in_memory(3000).expect("open memory");
        lam_storage::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    fn at() -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 2, 9, 0, 0)
            .single()
            .expect("fixed time")
    }

    #[test]
    fn empty_query_rejected() {
        let conn = migrated_conn();
        let project = lam_core::ProjectId::new();
        assert!(multi_search(&conn, &project, "   ", 5, at()).is_err());
        assert!(multi_search(&conn, &project, "x", 0, at()).is_err());
    }
}
