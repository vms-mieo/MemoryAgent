use std::collections::HashSet;

pub struct HybridVector<'a> {
    pub query: &'a [f32],
    pub model_id: &'a str,
    pub dimensions: usize,
    pub top_k: usize,
}

pub struct HybridResult {
    pub grouped: crate::types::GroupedHits,
    pub fused: Vec<crate::fusion::FusedHit>,
    pub vector_model: Option<String>,
    pub vector_warnings: Vec<String>,
}

pub fn hybrid_search(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    query: &str,
    per_source: usize,
    vector: Option<HybridVector>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<HybridResult, lam_core::LamError> {
    let grouped = crate::multi_search(conn, project_id, query, per_source, now)?;
    let mut vector_warnings = Vec::new();
    let mut vector_model = None;
    let mut vector_lists: Vec<Vec<String>> = Vec::new();
    if let Some(spec) = vector {
        if spec.dimensions == 0 || spec.query.len() != spec.dimensions {
            vector_warnings
                .push("vector query dimensions mismatch; vector ranks skipped".to_string());
        } else {
            match lam_storage::nearest_neighbors(
                conn,
                "chunk",
                spec.query,
                spec.model_id,
                spec.top_k.clamp(1, 20),
            ) {
                Ok(hits) => {
                    vector_model = Some(spec.model_id.to_string());
                    vector_lists.push(hits.into_iter().map(|(id, _)| id).collect());
                }
                Err(e) => {
                    vector_warnings.push(format!("vector search unavailable: {}", e.code()));
                }
            }
        }
    }
    let verified: HashSet<String> = grouped
        .memories
        .iter()
        .filter(|hit| hit.status.as_deref() == Some("verified"))
        .map(|hit| hit.id.clone())
        .collect();
    let fused = crate::fusion::fuse(&grouped, &vector_lists, &verified, query);
    Ok(HybridResult {
        grouped,
        fused,
        vector_model,
        vector_warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn fts_only_hybrid_works_without_vectors() {
        let mut conn = lam_storage::open_in_memory(3000).expect("open memory");
        lam_storage::apply_migrations(&mut conn).expect("migrate");
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 11, 2, 9, 0, 0)
            .single()
            .expect("fixed time");
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        let project_id =
            lam_storage::resolve_or_create(&mut conn, leaked.path(), Some("hybrid-proj"), now)
                .expect("project")
                .id;
        let result = hybrid_search(&conn, &project_id, "nothing matches this", 5, None, now)
            .expect("search");
        assert!(result.fused.is_empty());
        assert!(result.vector_model.is_none());
    }
}
