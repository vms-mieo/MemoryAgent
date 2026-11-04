use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone, Default)]
pub struct DreamCluster {
    pub project_id: lam_core::ProjectId,
    pub topic: String,
    pub episode_ids: Vec<String>,
    pub session_ids: Vec<String>,
    pub max_rowid: i64,
    pub count: usize,
    pub idle_minutes: i64,
}

#[derive(Debug, Clone)]
pub struct DreamOutcome {
    pub status: String,
    pub memory_id: Option<String>,
    pub support_links: Vec<String>,
    pub reason: String,
}

fn stopwords() -> &'static [&'static str] {
    &[
        "the",
        "a",
        "an",
        "and",
        "or",
        "to",
        "of",
        "for",
        "with",
        "on",
        "in",
        "is",
        "was",
        "are",
        "were",
        "be",
        "been",
        "by",
        "as",
        "at",
        "from",
        "that",
        "this",
        "it",
        "its",
        "which",
        "have",
        "has",
        "had",
        "will",
        "would",
        "can",
        "could",
        "should",
        "may",
        "you",
        "your",
        "they",
        "their",
        "we",
        "our",
        "he",
        "she",
        "not",
        "but",
        "if",
        "then",
        "than",
        "so",
        "such",
        "into",
        "over",
        "under",
        "about",
        "after",
        "before",
        "session",
        "completed",
        "started",
        "finished",
        "next",
        "current",
    ]
}

fn tokens_of(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| token.len() >= 4)
        .map(|token| token.to_lowercase())
        .filter(|token| !stopwords().contains(&token.as_str()))
        .collect()
}

fn last_dreamed_max(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    topic: &str,
) -> Result<i64, lam_core::LamError> {
    let prefix = format!("dream:{}:{topic}:", project_id);
    let mut stmt = conn
        .prepare(
            "SELECT input_ref FROM maintenance_jobs WHERE kind = 'dream_batch' AND input_ref LIKE ?1 AND state = 'done'",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![format!("{prefix}%")], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut max_seen = 0i64;
    for row in rows {
        let input_ref = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if let Some(tail) = input_ref.rsplit(':').next() {
            if let Ok(value) = tail.parse::<i64>() {
                if value > max_seen {
                    max_seen = value;
                }
            }
        }
    }
    Ok(max_seen)
}

pub fn find_eligible_clusters(
    conn: &rusqlite::Connection,
    min_new: usize,
    min_idle_minutes: i64,
    now: DateTime<Utc>,
) -> Result<Vec<DreamCluster>, lam_core::LamError> {
    let mut projects_stmt = conn
        .prepare(
            "SELECT DISTINCT s.project_id FROM episodes e JOIN sessions s ON s.id = e.session_id",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let project_rows = projects_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let mut project_ids = Vec::new();
    for row in project_rows {
        let text = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        project_ids.push(
            text.parse::<lam_core::ProjectId>().map_err(|_| {
                lam_core::LamError::storage("stored project id invalid".to_string())
            })?,
        );
    }
    let mut out = Vec::new();
    for project_id in project_ids {
        let mut topic_stmt = conn
            .prepare(
                "SELECT DISTINCT e.topics FROM episodes e
                 JOIN sessions s ON s.id = e.session_id
                 WHERE s.project_id = ?1",
            )
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let topic_rows = topic_stmt
            .query_map(rusqlite::params![project_id.to_string_value()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        for topic_row in topic_rows {
            let topic = topic_row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            let done_max = last_dreamed_max(conn, &project_id, &topic)?;
            let mut episode_stmt = conn
                .prepare(
                    "SELECT e.id, e.session_id, e.rowid FROM episodes e
                     JOIN sessions s ON s.id = e.session_id
                     WHERE s.project_id = ?1 AND e.topics = ?2 AND e.rowid > ?3
                     ORDER BY e.rowid ASC",
                )
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            let episode_rows = episode_stmt
                .query_map(
                    rusqlite::params![project_id.to_string_value(), topic, done_max],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
            let mut episode_ids = Vec::new();
            let mut session_ids = Vec::new();
            let mut max_rowid = done_max;
            for row in episode_rows {
                let (id, session, rowid) =
                    row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
                episode_ids.push(id);
                if !session_ids.contains(&session) {
                    session_ids.push(session);
                }
                if rowid > max_rowid {
                    max_rowid = rowid;
                }
            }
            if episode_ids.is_empty() {
                continue;
            }
            let idle_minutes = idle_minutes_since(conn, &project_id, now)?;
            if episode_ids.len() >= min_new.max(2)
                || (min_idle_minutes > 0
                    && idle_minutes >= min_idle_minutes
                    && episode_ids.len() >= 2)
            {
                out.push(DreamCluster {
                    project_id,
                    topic,
                    episode_ids,
                    session_ids,
                    max_rowid,
                    count: 0,
                    idle_minutes,
                });
            }
        }
    }
    for cluster in out.iter_mut() {
        cluster.count = cluster.episode_ids.len();
    }
    Ok(out)
}

fn idle_minutes_since(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    now: DateTime<Utc>,
) -> Result<i64, lam_core::LamError> {
    let latest: Option<String> = conn
        .query_row(
            "SELECT MAX(s.started_at) FROM sessions s WHERE s.project_id = ?1",
            rusqlite::params![project_id.to_string_value()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    match latest {
        Some(text) => {
            let recorded = text.parse::<DateTime<Utc>>().map_err(|_| {
                lam_core::LamError::storage("stored episode time invalid".to_string())
            })?;
            Ok((now - recorded).num_minutes().max(0))
        }
        None => Ok(i64::MAX / 2),
    }
}

fn episode_summaries(
    conn: &rusqlite::Connection,
    episode_ids: &[String],
) -> Result<Vec<(String, String)>, lam_core::LamError> {
    let mut out = Vec::new();
    for id in episode_ids {
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT abstract, summary FROM episodes WHERE id = ?1",
                rusqlite::params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        if let Some(found) = row {
            out.push((id.clone(), format!("{} {}", found.0, found.1)));
        }
    }
    Ok(out)
}

fn shared_phrases(summaries: &[String]) -> Vec<(String, usize, Vec<usize>)> {
    let mut counts: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (index, text) in summaries.iter().enumerate() {
        let tokens = tokens_of(text);
        let mut seen_in_doc: std::collections::HashSet<String> = std::collections::HashSet::new();
        for width in 2..=4usize {
            if tokens.len() < width {
                continue;
            }
            for window in tokens.windows(width) {
                let phrase = window.join(" ");
                if seen_in_doc.insert(phrase.clone()) {
                    counts.entry(phrase).or_default().push(index);
                }
            }
        }
    }
    let mut ranked: Vec<(String, usize, Vec<usize>)> = counts
        .into_iter()
        .filter(|(_, docs)| docs.len() >= 2)
        .map(|(phrase, mut docs)| {
            docs.sort_unstable();
            docs.dedup();
            let support = docs.len();
            (phrase, support, docs)
        })
        .filter(|(_, support, _)| *support >= 2)
        .collect();
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.0.len().cmp(&a.0.len()))
            .then_with(|| a.0.cmp(&b.0))
    });
    ranked
}

pub fn synthesize(
    conn: &mut rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    cluster: &DreamCluster,
    now: DateTime<Utc>,
) -> Result<DreamOutcome, lam_core::LamError> {
    let summaries = episode_summaries(conn, &cluster.episode_ids)?;
    if summaries.len() < 2 {
        return Ok(DreamOutcome {
            status: "deferred".to_string(),
            memory_id: None,
            support_links: Vec::new(),
            reason: "needs at least 2 episodes".to_string(),
        });
    }
    let mut distinct_sessions = cluster.session_ids.clone();
    distinct_sessions.sort();
    distinct_sessions.dedup();
    if distinct_sessions.len() < 2 {
        return Ok(DreamOutcome {
            status: "deferred".to_string(),
            memory_id: None,
            support_links: Vec::new(),
            reason: "needs at least 2 independent session groups".to_string(),
        });
    }
    let texts: Vec<String> = summaries.iter().map(|(_, text)| text.clone()).collect();
    let phrases = shared_phrases(&texts);
    let Some((phrase, support, doc_indexes)) = phrases.into_iter().next() else {
        return Ok(DreamOutcome {
            status: "deferred".to_string(),
            memory_id: None,
            support_links: Vec::new(),
            reason: "no shared phrase across episodes".to_string(),
        });
    };
    let _ = support;
    if lam_core::secrets::find_secret_kind(&phrase).is_some() {
        return Ok(DreamOutcome {
            status: "deferred".to_string(),
            memory_id: None,
            support_links: Vec::new(),
            reason: "candidate phrase looks sensitive".to_string(),
        });
    }
    if known_verified_contains(conn, project_id, &phrase)? {
        return Ok(DreamOutcome {
            status: "discarded".to_string(),
            memory_id: None,
            support_links: Vec::new(),
            reason: "already known as verified memory".to_string(),
        });
    }
    let support_ids: Vec<String> = doc_indexes
        .iter()
        .filter_map(|index| summaries.get(*index).map(|(id, _)| id.clone()))
        .collect();
    let statement = format!(
        "Recurring theme across {} episodes ({}): \"{}\"",
        support_ids.len(),
        cluster.topic,
        phrase
    );
    let confidence = (0.5 + 0.05 * support_ids.len() as f64).min(0.85);
    let (memory_id, _) = lam_storage::save_memory(
        conn,
        lam_storage::SaveInput {
            project_id: *project_id,
            scope: lam_core::Scope::Project,
            kind: lam_core::MemoryKind::Lesson,
            content: statement.clone(),
            status: lam_core::MemoryStatus::Proposed,
            actor: "dream:batch".to_string(),
            reason: Some("dream synthesis; needs human review".to_string()),
            sources: support_ids
                .iter()
                .map(|episode_id| lam_core::SourceEvidence {
                    source_kind: lam_core::SourceKind::Episode,
                    source_ref: episode_id.clone(),
                    line_start: None,
                    line_end: None,
                    commit_sha: None,
                    content_hash: None,
                })
                .collect(),
            idempotency_key: Some(format!(
                "dream:{}:{}:{}",
                project_id.to_string_value(),
                cluster.topic,
                cluster.max_rowid
            )),
        },
        now,
    )
    .map_err(|e| {
        if e.code() == "E_FACT_CONFLICT" {
            lam_core::LamError::validation("E_DREAM_DEFERRED", e.to_string())
        } else {
            e
        }
    })?;
    Ok(DreamOutcome {
        status: "proposed".to_string(),
        memory_id: Some(memory_id.to_string()),
        support_links: support_ids,
        reason: format!(
            "confidence {confidence:.2}; independent sources: {}",
            distinct_sessions.len()
        ),
    })
}

fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn known_verified_contains(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    phrase: &str,
) -> Result<bool, lam_core::LamError> {
    let pattern = format!("%{}%", escape_like(&phrase.to_lowercase()));
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories m
             JOIN memory_revisions r ON r.id = m.current_revision
             WHERE m.project_id = ?1 AND r.status = 'verified' AND lower(r.content) LIKE ?2 ESCAPE '\\'",
            rusqlite::params![project_id.to_string_value(), pattern],
            |row| row.get(0),
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    if count > 0 {
        return Ok(true);
    }
    let phrase_tokens: Vec<String> = tokens_of(phrase);
    if phrase_tokens.len() < 2 {
        return Ok(false);
    }
    let mut stmt = conn
        .prepare(
            "SELECT lower(r.content) FROM memories m
             JOIN memory_revisions r ON r.id = m.current_revision
             WHERE m.project_id = ?1 AND r.status = 'verified'",
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![project_id.to_string_value()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for row in rows {
        let content = row.map_err(|e| lam_core::LamError::storage(e.to_string()))?;
        let hits = phrase_tokens
            .iter()
            .filter(|token| content.contains(token.as_str()))
            .count();
        if hits >= 2 && hits * 2 >= phrase_tokens.len() {
            return Ok(true);
        }
    }
    Ok(false)
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
        let mut conn = lam_storage::open_in_memory(3000).expect("open memory");
        lam_storage::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    fn at() -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 2, 9, 0, 0)
            .single()
            .expect("fixed time")
    }

    fn seed_episode(
        conn: &rusqlite::Connection,
        session: &str,
        abstract_text: &str,
        summary: &str,
        topics: &str,
    ) {
        let session_id: String = conn
            .query_row(
                "SELECT id FROM sessions WHERE external_session_id = ?1",
                rusqlite::params![session],
                |row| row.get(0),
            )
            .optional()
            .expect("lookup")
            .unwrap_or_else(|| {
                let project: String = conn
                    .query_row("SELECT id FROM projects LIMIT 1", [], |row| row.get(0))
                    .expect("project");
                let id = lam_core::SessionId::new().to_string();
                conn.execute(
                    "INSERT INTO sessions (id, project_id, agent_kind, external_session_id, branch, started_at, ended_at)
                     VALUES (?1, ?2, 'cli', ?3, NULL, ?4, NULL)",
                    rusqlite::params![id, project, session, at().to_rfc3339()],
                )
                .expect("session");
                id
            });
        let episode_id = lam_core::RevisionId::new().to_string();
        conn.execute(
            "INSERT INTO episodes (id, session_id, abstract, summary, topics, quality, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0.5, ?6)",
            rusqlite::params![
                episode_id,
                session_id,
                abstract_text,
                summary,
                topics,
                "2027-11-02T09:00:00+00:00"
            ],
        )
        .expect("episode");
        conn.execute(
            "INSERT INTO episode_fts (episode_id, abstract, summary, topics) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![episode_id, abstract_text, summary, topics],
        )
        .expect("fts");
    }

    fn temp_project(conn: &mut rusqlite::Connection) -> lam_core::ProjectId {
        let dir = tempfile::tempdir().expect("tempdir");
        let leaked = Box::leak(Box::new(dir));
        lam_storage::resolve_or_create(conn, leaked.path(), Some("dream-proj"), at())
            .expect("project")
            .id
    }

    #[test]
    fn related_episodes_yield_one_proposed_lesson() {
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn);
        for index in 0..3 {
            seed_episode(
                &conn,
                &format!("debug-session-{index}"),
                "Debugging Vitastor boot failure",
                "Checked udp timeout handling in media path; boot still failing on staging",
                "debug",
            );
        }
        let clusters = find_eligible_clusters(&conn, 2, 10_000, at()).expect("clusters");
        assert_eq!(clusters.len(), 1);
        let outcome = synthesize(&mut conn, &project_id, &clusters[0], at()).expect("dream");
        assert_eq!(outcome.status, "proposed");
        let memory_id = outcome.memory_id.expect("memory id");
        assert!(outcome.support_links.len() >= 2);
        let status: String = conn
            .query_row(
                "SELECT r.status FROM memory_revisions r JOIN memories m ON m.id = r.memory_id WHERE m.id = ?1",
                rusqlite::params![memory_id],
                |row| row.get(0),
            )
            .expect("status");
        assert_eq!(status, "proposed");
    }

    #[test]
    fn single_source_cluster_stays_hypothesis_free() {
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn);
        for _ in 0..3 {
            seed_episode(
                &conn,
                "lonely-session",
                "Debugging Vitastor boot failure",
                "Checked udp timeout handling in media path",
                "debug",
            );
        }
        let clusters = find_eligible_clusters(&conn, 2, 10_000, at()).expect("clusters");
        assert_eq!(clusters.len(), 1);
        let outcome = synthesize(&mut conn, &project_id, &clusters[0], at()).expect("dream");
        assert_eq!(outcome.status, "deferred");
        assert!(outcome.memory_id.is_none());
    }

    #[test]
    fn verified_knowledge_is_not_relearned() {
        let mut conn = migrated_conn();
        let project_id = temp_project(&mut conn);
        lam_storage::save_memory(
            &mut conn,
            lam_storage::SaveInput {
                project_id,
                scope: lam_core::Scope::Project,
                kind: lam_core::MemoryKind::ArchitectureDecision,
                content: "Decision: use Temporal for orchestration".to_string(),
                status: lam_core::MemoryStatus::Verified,
                actor: "cli".to_string(),
                reason: None,
                sources: vec![],
                idempotency_key: None,
            },
            at(),
        )
        .expect("adr");
        for index in 0..3 {
            seed_episode(
                &conn,
                &format!("adr-session-{index}"),
                "Following the Temporal decision",
                "We should use Temporal for orchestration everywhere",
                "adr",
            );
        }
        let clusters = find_eligible_clusters(&conn, 2, 10_000, at()).expect("clusters");
        let cluster = clusters
            .iter()
            .find(|c| c.topic == "adr")
            .expect("adr cluster");
        let outcome = synthesize(&mut conn, &project_id, cluster, at()).expect("dream");
        assert!(outcome.status == "deferred" || outcome.status == "discarded");
        assert!(outcome.memory_id.is_none());
    }
}
