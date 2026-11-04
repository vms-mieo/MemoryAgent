use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct EntityView {
    pub id: lam_core::EntityId,
    pub canonical_name: String,
    pub entity_type: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EdgeView {
    pub id: String,
    pub source_id: String,
    pub source_name: String,
    pub relation_type: String,
    pub target_id: String,
    pub target_name: String,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct TraversalView {
    pub nodes: Vec<EntityView>,
    pub edges: Vec<EdgeView>,
}

fn parse_time(text: &str) -> Result<DateTime<Utc>, lam_core::LamError> {
    text.parse::<DateTime<Utc>>()
        .map_err(|_| lam_core::LamError::storage("graph row has invalid time".to_string()))
}

pub fn upsert_entity(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    canonical_name: &str,
    entity_type: &str,
    metadata: Option<serde_json::Value>,
) -> Result<lam_core::EntityId, lam_core::LamError> {
    let name = canonical_name.trim();
    if name.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_ENTITY_NAME_INVALID",
            "canonical name must not be empty".to_string(),
        ));
    }
    if name.len() > 512 {
        return Err(lam_core::LamError::validation(
            "E_ENTITY_NAME_INVALID",
            "canonical name exceeds 512 chars".to_string(),
        ));
    }
    let kind: lam_core::EntityType = entity_type.parse()?;
    let meta = metadata.unwrap_or(serde_json::json!({}));
    let meta_text = serde_json::to_string(&meta).map_err(lam_core::LamError::from)?;
    if meta_text.len() > 8192 {
        return Err(lam_core::LamError::validation(
            "E_ENTITY_METADATA_INVALID",
            "metadata exceeds 8192 bytes".to_string(),
        ));
    }
    if let Some(existing) = find_entity(conn, project_id, name)? {
        return Ok(existing);
    }
    let id = lam_core::EntityId::new();
    conn.execute(
        "INSERT INTO entities (id, project_id, canonical_name, entity_type, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            id.to_string(),
            project_id.to_string_value(),
            name,
            kind.as_str(),
            meta_text,
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(id)
}

pub fn find_entity(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    name: &str,
) -> Result<Option<lam_core::EntityId>, lam_core::LamError> {
    let by_name: Option<String> = conn
        .query_row(
            "SELECT id FROM entities WHERE project_id = ?1 AND canonical_name = ?2",
            rusqlite::params![project_id.to_string_value(), name.trim()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if let Some(text) = by_name {
        return text
            .parse::<lam_core::EntityId>()
            .map(Some)
            .map_err(|_| lam_core::LamError::storage("stored entity id invalid".to_string()));
    }
    let by_alias: Option<String> = conn
        .query_row(
            "SELECT e.id FROM entities e
             JOIN entity_aliases a ON a.entity_id = e.id
             WHERE e.project_id = ?1 AND a.alias = ?2",
            rusqlite::params![project_id.to_string_value(), name.trim()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    match by_alias {
        Some(text) => text
            .parse::<lam_core::EntityId>()
            .map(Some)
            .map_err(|_| lam_core::LamError::storage("stored entity id invalid".to_string())),
        None => Ok(None),
    }
}

fn entity_project(
    conn: &rusqlite::Connection,
    entity_id: &lam_core::EntityId,
) -> Result<Option<lam_core::ProjectId>, lam_core::LamError> {
    let found: Option<String> = conn
        .query_row(
            "SELECT project_id FROM entities WHERE id = ?1",
            rusqlite::params![entity_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    match found {
        Some(text) => text
            .parse::<lam_core::ProjectId>()
            .map(Some)
            .map_err(|_| lam_core::LamError::storage("stored project id invalid".to_string())),
        None => Ok(None),
    }
}

pub fn attach_alias(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    entity_id: &lam_core::EntityId,
    alias: &str,
) -> Result<(), lam_core::LamError> {
    let clean = alias.trim();
    if clean.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_ALIAS_INVALID",
            "alias must not be empty".to_string(),
        ));
    }
    let owner = entity_project(conn, entity_id)?.ok_or_else(|| {
        lam_core::LamError::validation("E_ENTITY_NOT_FOUND", format!("no entity {entity_id}"))
    })?;
    if owner != *project_id {
        return Err(lam_core::LamError::validation(
            "E_SCOPE_DENIED",
            "entity belongs to another project".to_string(),
        ));
    }
    if let Some(other) = find_entity(conn, project_id, clean)? {
        if other != *entity_id {
            return Err(lam_core::LamError::validation(
                "E_ALIAS_CONFLICT",
                format!("{clean} already identifies another entity"),
            ));
        }
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO entity_aliases (entity_id, alias) VALUES (?1, ?2)",
        rusqlite::params![entity_id.to_string(), clean],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(())
}

#[derive(Debug, Clone)]
pub struct EdgeInput<'a> {
    pub project_id: &'a lam_core::ProjectId,
    pub source_id: &'a lam_core::EntityId,
    pub relation_type: &'a str,
    pub target_id: &'a lam_core::EntityId,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub source_ref: Option<&'a str>,
    pub now: DateTime<Utc>,
}

pub fn add_edge(
    conn: &rusqlite::Connection,
    input: EdgeInput,
) -> Result<lam_core::RelationId, lam_core::LamError> {
    let kind: lam_core::RelationType = input.relation_type.parse()?;
    if input.source_id == input.target_id {
        return Err(lam_core::LamError::validation(
            "E_RELATION_INVALID",
            "relation source and target must differ".to_string(),
        ));
    }
    for endpoint in [input.source_id, input.target_id] {
        match entity_project(conn, endpoint)? {
            Some(owner) if owner == *input.project_id => {}
            Some(_) => {
                return Err(lam_core::LamError::validation(
                    "E_SCOPE_DENIED",
                    "relation endpoint belongs to another project".to_string(),
                ));
            }
            None => {
                return Err(lam_core::LamError::validation(
                    "E_ENTITY_NOT_FOUND",
                    format!("no entity {endpoint}"),
                ));
            }
        }
    }
    if let Some(until) = input.valid_until {
        if input.valid_from > until {
            return Err(lam_core::LamError::validation(
                "E_RELATION_VALIDITY_INVALID",
                "valid_from must not exceed valid_until".to_string(),
            ));
        }
    }
    let id = lam_core::RelationId::new();
    conn.execute(
        "INSERT INTO relations (id, source_entity_id, relation_type, target_entity_id, valid_from, valid_until, source_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            id.to_string(),
            input.source_id.to_string(),
            kind.as_str(),
            input.target_id.to_string(),
            input.valid_from.to_rfc3339(),
            input.valid_until.map(|t| t.to_rfc3339()),
            input.source_ref.map(|s| s.trim().to_string()),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(id)
}

pub fn entity_detail(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    entity_id: &lam_core::EntityId,
) -> Result<EntityView, lam_core::LamError> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT canonical_name, entity_type FROM entities WHERE id = ?1 AND project_id = ?2",
            rusqlite::params![entity_id.to_string(), project_id.to_string_value()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let (name, kind) = row.ok_or_else(|| {
        lam_core::LamError::validation(
            "E_ENTITY_NOT_FOUND",
            format!("no entity {entity_id} in project"),
        )
    })?;
    let mut stmt = conn
        .prepare("SELECT alias FROM entity_aliases WHERE entity_id = ?1 ORDER BY alias ASC")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let aliases = stmt
        .query_map(rusqlite::params![entity_id.to_string()], |row| row.get(0))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    Ok(EntityView {
        id: *entity_id,
        canonical_name: name,
        entity_type: kind,
        aliases,
    })
}

pub fn search_entities(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    query: &str,
    limit: usize,
) -> Result<Vec<EntityView>, lam_core::LamError> {
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
    let tokens: Vec<String> = trimmed
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|token| token.len() >= 2)
        .map(|token| token.to_string())
        .collect();
    if tokens.is_empty() {
        return Err(lam_core::LamError::validation(
            "E_QUERY_INVALID",
            "query has no searchable tokens".to_string(),
        ));
    }
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    params.push(project_id.to_string_value());
    for token in &tokens {
        let escaped = token
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        conditions.push(
            "(e.canonical_name LIKE ? ESCAPE '\\' OR a.alias LIKE ? ESCAPE '\\')".to_string(),
        );
        params.push(pattern.clone());
        params.push(pattern);
    }
    params.push((limit as i64).to_string());
    let sql = format!(
        "SELECT DISTINCT e.id FROM entities e
         LEFT JOIN entity_aliases a ON a.entity_id = e.id
         WHERE e.project_id = ?1 AND ({})
         ORDER BY e.canonical_name ASC LIMIT ?{}",
        conditions.join(" OR "),
        params.len()
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        let text = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let id: lam_core::EntityId = text
            .parse()
            .map_err(|_| lam_core::LamError::storage("stored entity id invalid".to_string()))?;
        out.push(entity_detail(conn, project_id, &id)?);
    }
    Ok(out)
}

pub fn traverse(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    start: &lam_core::EntityId,
    depth: u8,
    as_of: DateTime<Utc>,
) -> Result<TraversalView, lam_core::LamError> {
    if depth == 0 || depth > 3 {
        return Err(lam_core::LamError::validation(
            "E_DEPTH_INVALID",
            "traversal depth must be 1, 2, or 3".to_string(),
        ));
    }
    entity_detail(conn, project_id, start)?;
    let mut seen = std::collections::HashSet::new();
    seen.insert(start.to_string());
    let mut frontier = vec![*start];
    let mut nodes = vec![entity_detail(conn, project_id, start)?];
    let mut edges = Vec::new();
    let stamp = as_of.to_rfc3339();
    for _ in 0..depth {
        let mut next = Vec::new();
        for node in &frontier {
            let mut stmt = conn
                .prepare(
                    "SELECT r.id, r.source_entity_id, s.canonical_name, r.relation_type,
                            r.target_entity_id, t.canonical_name, r.valid_from, r.valid_until
                     FROM relations r
                     JOIN entities s ON s.id = r.source_entity_id
                     JOIN entities t ON t.id = r.target_entity_id
                     WHERE (r.source_entity_id = ?1 OR r.target_entity_id = ?1)
                       AND s.project_id = ?2 AND t.project_id = ?2
                       AND r.valid_from <= ?3 AND (r.valid_until IS NULL OR ?3 < r.valid_until)",
                )
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            let rows = stmt
                .query_map(
                    rusqlite::params![node.to_string(), project_id.to_string_value(), stamp],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, Option<String>>(7)?,
                        ))
                    },
                )
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            for row in rows {
                let tuple = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                let bad = || lam_core::LamError::storage("graph row has invalid value".to_string());
                edges.push(EdgeView {
                    id: tuple.0,
                    source_id: tuple.1,
                    source_name: tuple.2,
                    relation_type: tuple.3,
                    target_id: tuple.4,
                    target_name: tuple.5,
                    valid_from: parse_time(&tuple.6).map_err(|_| bad())?,
                    valid_until: tuple
                        .7
                        .map(|text| parse_time(&text))
                        .transpose()
                        .map_err(|_| bad())?,
                });
                let (source_text, target_text) = {
                    let last = edges.last().expect("edge pushed");
                    (last.source_id.clone(), last.target_id.clone())
                };
                for text in [source_text, target_text] {
                    if seen.insert(text.clone()) {
                        if let Ok(id) = text.parse::<lam_core::EntityId>() {
                            if entity_detail(conn, project_id, &id).is_ok() {
                                next.push(id);
                                nodes.push(entity_detail(conn, project_id, &id)?);
                            }
                        }
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(TraversalView { nodes, edges })
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

    fn at(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, year, month, day, 9, 0, 0)
            .single()
            .expect("fixed time")
    }

    fn test_project(conn: &mut rusqlite::Connection) -> lam_core::ProjectId {
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        crate::projects::resolve_or_create(conn, leaked.path(), Some("graph-proj"), at(2026, 11, 2))
            .expect("project")
            .id
    }

    #[test]
    fn alias_conflict_is_rejected() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let kafka = upsert_entity(&conn, &project_id, "Kafka", "service", None).expect("kafka");
        let cluster =
            upsert_entity(&conn, &project_id, "kafka-cluster", "service", None).expect("cluster");
        attach_alias(&conn, &project_id, &kafka, "queue").expect("alias");
        let conflict = attach_alias(&conn, &project_id, &cluster, "queue");
        assert!(conflict.is_err());
        assert_eq!(conflict.unwrap_err().code(), "E_ALIAS_CONFLICT");
    }

    #[test]
    fn traversal_respects_validity_window() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let now = at(2026, 11, 2);
        let api = upsert_entity(&conn, &project_id, "api", "service", None).expect("api");
        let db = upsert_entity(&conn, &project_id, "postgres", "database", None).expect("db");
        add_edge(
            &conn,
            EdgeInput {
                project_id: &project_id,
                source_id: &api,
                relation_type: "depends_on",
                target_id: &db,
                valid_from: at(2026, 10, 1),
                valid_until: Some(at(2026, 10, 20)),
                source_ref: None,
                now,
            },
        )
        .expect("edge");
        let during = traverse(&conn, &project_id, &api, 1, at(2026, 10, 10)).expect("traverse");
        assert_eq!(during.edges.len(), 1);
        let after = traverse(&conn, &project_id, &api, 1, at(2026, 10, 21)).expect("traverse");
        assert!(after.edges.is_empty());
        assert_eq!(after.nodes.len(), 1);
    }

    #[test]
    fn cross_project_endpoints_are_rejected() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let other_dir = tempfile::tempdir().expect("tempdir");
        let other_leaked = Box::leak(Box::new(other_dir));
        let other_id = crate::projects::resolve_or_create(
            &mut conn,
            other_leaked.path(),
            Some("other"),
            at(2026, 11, 2),
        )
        .expect("other")
        .id;
        let now = at(2026, 11, 2);
        let mine = upsert_entity(&conn, &project_id, "mine", "service", None).expect("mine");
        let theirs = upsert_entity(&conn, &other_id, "theirs", "service", None).expect("theirs");
        let result = add_edge(
            &conn,
            EdgeInput {
                project_id: &project_id,
                source_id: &mine,
                relation_type: "uses",
                target_id: &theirs,
                valid_from: now,
                valid_until: None,
                source_ref: None,
                now,
            },
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "E_SCOPE_DENIED");
    }

    #[test]
    fn depth_is_bounded() {
        let mut conn = migrated_conn();
        let project_id = test_project(&mut conn);
        let now = at(2026, 11, 2);
        let root = upsert_entity(&conn, &project_id, "root", "service", None).expect("root");
        assert!(traverse(&conn, &project_id, &root, 0, now).is_err());
        assert!(traverse(&conn, &project_id, &root, 4, now).is_err());
        assert!(traverse(&conn, &project_id, &root, 2, now).is_ok());
    }
}
