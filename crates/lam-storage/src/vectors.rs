use chrono::Utc;

#[derive(Debug, Clone)]
pub struct ModelRecord {
    pub model_id: String,
    pub dimensions: usize,
    pub status: String,
}

pub fn ensure_model(
    conn: &rusqlite::Connection,
    model_id: &str,
    dimensions: usize,
) -> Result<(ModelRecord, u64), lam_core::LamError> {
    if dimensions == 0 || dimensions > 4096 {
        return Err(lam_core::LamError::validation(
            "E_DIM_INVALID",
            "dimensions must be within 1 and 4096".to_string(),
        ));
    }
    let now = Utc::now().to_rfc3339();
    let existing: Option<(i64, String)> = conn
        .query_row(
            "SELECT dimensions, status FROM embedding_models WHERE model_id = ?1",
            rusqlite::params![model_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    match existing {
        None => {
            conn.execute(
                "INSERT INTO embedding_models (model_id, dimensions, status, created_at)
                 VALUES (?1, ?2, 'active', ?3)",
                rusqlite::params![model_id, dimensions as i64, now],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        }
        Some((stored, _)) if stored as usize == dimensions => {
            conn.execute(
                "UPDATE embedding_models SET status = 'active' WHERE model_id = ?1",
                rusqlite::params![model_id],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        }
        Some(_) => {
            let dropped = clear_model_vectors(conn, model_id)?;
            conn.execute(
                "UPDATE embedding_models SET dimensions = ?1, status = 'active', created_at = ?2 WHERE model_id = ?3",
                rusqlite::params![dimensions as i64, now, model_id],
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            crate::db::harden_current_db(conn);
            return Ok((
                ModelRecord {
                    model_id: model_id.to_string(),
                    dimensions,
                    status: "active".to_string(),
                },
                dropped,
            ));
        }
    }
    crate::db::harden_current_db(conn);
    Ok((
        ModelRecord {
            model_id: model_id.to_string(),
            dimensions,
            status: "active".to_string(),
        },
        0,
    ))
}

pub fn encode_vector(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub fn decode_vector(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.is_empty() || !blob.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(blob.len() / 4);
    let (chunks, _) = blob.as_chunks::<4>();
    for chunk in chunks {
        let value = f32::from_le_bytes(*chunk);
        if !value.is_finite() {
            return None;
        }
        out.push(value);
    }
    Some(out)
}

pub fn store_vector(
    conn: &rusqlite::Connection,
    owner_kind: &str,
    owner_id: &str,
    model_id: &str,
    dimensions: usize,
    vector: &[f32],
) -> Result<(), lam_core::LamError> {
    if vector.len() != dimensions {
        return Err(lam_core::LamError::validation(
            "E_DIMENSION_MISMATCH",
            format!(
                "vector has {} dims, model {model_id} expects {dimensions}; reindex the vault",
                vector.len()
            ),
        ));
    }
    if vector.iter().any(|v| !v.is_finite()) {
        return Err(lam_core::LamError::validation(
            "E_VECTOR_INVALID",
            "vector must contain finite values".to_string(),
        ));
    }
    let id = blake3::hash(format!("lam-vector:v1:{owner_kind}\n{owner_id}\n{model_id}").as_bytes())
        .to_hex()
        .to_string();
    conn.execute(
        "DELETE FROM vector_index WHERE owner_kind = ?1 AND owner_id = ?2 AND embedding_model = ?3",
        rusqlite::params![owner_kind, owner_id, model_id],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    conn.execute(
        "INSERT INTO vector_index (id, owner_kind, owner_id, embedding_model, dimensions, vector, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            id,
            owner_kind,
            owner_id,
            model_id,
            dimensions as i64,
            encode_vector(vector),
            Utc::now().to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(())
}

pub fn load_vectors(
    conn: &rusqlite::Connection,
    owner_kind: &str,
    model_id: &str,
    dimensions: usize,
) -> Result<Vec<(String, Vec<f32>)>, lam_core::LamError> {
    let mut stmt = conn
        .prepare(
            "SELECT owner_id, vector FROM vector_index
             WHERE owner_kind = ?1 AND embedding_model = ?2 AND dimensions = ?3",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(
            rusqlite::params![owner_kind, model_id, dimensions as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let (owner_id, blob) = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if let Some(vector) = decode_vector(&blob) {
            if vector.len() == dimensions {
                out.push((owner_id, vector));
            }
        }
    }
    Ok(out)
}

pub fn clear_model_vectors(
    conn: &rusqlite::Connection,
    model_id: &str,
) -> Result<u64, lam_core::LamError> {
    let removed = conn
        .execute(
            "DELETE FROM vector_index WHERE embedding_model = ?1",
            rusqlite::params![model_id],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(removed as u64)
}

pub fn vector_count(
    conn: &rusqlite::Connection,
    owner_kind: &str,
    model_id: &str,
) -> Result<i64, lam_core::LamError> {
    conn.query_row(
        "SELECT COUNT(*) FROM vector_index WHERE owner_kind = ?1 AND embedding_model = ?2",
        rusqlite::params![owner_kind, model_id],
        |row| row.get(0),
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))
}

pub fn nearest_neighbors(
    conn: &rusqlite::Connection,
    owner_kind: &str,
    query: &[f32],
    model_id: &str,
    top_k: usize,
) -> Result<Vec<(String, f32)>, lam_core::LamError> {
    if query.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_QUERY_INVALID",
            "query vector must not be empty".to_string(),
        ));
    }
    let candidates = load_vectors(conn, owner_kind, model_id, query.len())?;
    let mut scored: Vec<(String, f32)> = candidates
        .into_iter()
        .filter_map(|(id, vector)| {
            lam_providers::cosine_similarity(query, &vector).map(|score| (id, score))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(top_k.clamp(1, 100));
    Ok(scored)
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

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    #[test]
    fn encode_decode_round_trip() {
        let vector = vec![0.5f32, -0.25, 1.0];
        let decoded = decode_vector(&encode_vector(&vector)).expect("decode");
        assert_eq!(decoded, vector);
        assert!(decode_vector(&[0u8; 3]).is_none());
        assert!(decode_vector(&encode_vector(&[f32::INFINITY])).is_none());
        assert!(decode_vector(&encode_vector(&[f32::NAN])).is_none());
    }

    #[test]
    fn dimension_mismatch_migrates_with_drop() {
        let conn = migrated_conn();
        ensure_model(&conn, "model-a", 4).expect("register");
        store_vector(&conn, "chunk", "c1", "model-a", 4, &[0.1, 0.2, 0.3, 0.4]).expect("store");
        assert_eq!(vector_count(&conn, "chunk", "model-a").expect("count"), 1);
        let (_, dropped) = ensure_model(&conn, "model-a", 8).expect("migrate");
        assert_eq!(dropped, 1);
        assert_eq!(vector_count(&conn, "chunk", "model-a").expect("count"), 0);
    }

    #[test]
    fn nearest_orders_by_cosine() {
        let conn = migrated_conn();
        ensure_model(&conn, "model-b", 2).expect("register");
        store_vector(&conn, "chunk", "near", "model-b", 2, &[1.0, 0.0]).expect("store");
        store_vector(&conn, "chunk", "far", "model-b", 2, &[0.0, 1.0]).expect("store");
        let ranked = nearest_neighbors(&conn, "chunk", &[0.9, 0.1], "model-b", 2).expect("search");
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, "near");
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn wrong_dims_rejected_on_store() {
        let conn = migrated_conn();
        ensure_model(&conn, "model-c", 2).expect("register");
        let result = store_vector(&conn, "chunk", "c1", "model-c", 2, &[0.1]);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "E_DIMENSION_MISMATCH");
    }
}
