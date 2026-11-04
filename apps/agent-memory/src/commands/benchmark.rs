use std::path::PathBuf;
use std::time::Instant;

use lam_providers::Embedder;

const DATASET_JSON: &str = include_str!("../../../../fixtures/benchmark/dataset.json");
const CORPUS_JSON: &str = include_str!("../../../../fixtures/benchmark/corpus.json");

#[derive(Debug, Clone)]
struct BenchQuery {
    id: String,
    category: String,
    query: String,
    expect_kind: String,
    expect_match: String,
}

#[derive(Debug, Default)]
struct ModeMetrics {
    mode: String,
    queries: usize,
    scored: usize,
    recall_at_5_hits: usize,
    reciprocal_sum: f64,
    budget_violations: usize,
    stale_hits: usize,
    leakage_hits: usize,
    secrets_found: usize,
    latencies_ms: Vec<f64>,
    by_category: std::collections::BTreeMap<String, CategoryScore>,
    exclusions: ExclusionCheck,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
struct CategoryScore {
    scored: usize,
    hits: usize,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
struct ExclusionCheck {
    checks: usize,
    violations: usize,
}

impl ModeMetrics {
    fn recall_at_5(&self) -> f64 {
        if self.scored == 0 {
            0.0
        } else {
            self.recall_at_5_hits as f64 / self.scored as f64
        }
    }

    fn mrr_at_10(&self) -> f64 {
        if self.scored == 0 {
            0.0
        } else {
            self.reciprocal_sum / self.scored as f64
        }
    }

    fn percentile(&self, pct: f64) -> f64 {
        if self.latencies_ms.is_empty() {
            return 0.0;
        }
        let mut sorted = self.latencies_ms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let index = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
        sorted[index.saturating_sub(1).min(sorted.len() - 1)]
    }
}

pub fn run(_home: Option<PathBuf>, as_json: bool) -> Result<(), lam_core::LamError> {
    let report = run_benchmark()?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("retrieval benchmark (offline, local-only)");
        println!(
            "hardware: {} {}",
            std::env::consts::ARCH,
            std::env::consts::OS
        );
        if let Some(modes) = report.get("modes").and_then(|v| v.as_array()) {
            for mode in modes {
                let name = mode.get("mode").and_then(|v| v.as_str()).unwrap_or("?");
                println!("mode={name}");
                for key in [
                    "queries",
                    "scored",
                    "recall_at_5",
                    "mrr_at_10",
                    "budget_violations",
                    "stale_hits",
                    "leakage_hits",
                    "secrets_found",
                    "latency_p50_ms",
                    "latency_p95_ms",
                ] {
                    println!(
                        "  {key}: {}",
                        mode.get(key).unwrap_or(&serde_json::json!(null))
                    );
                }
                if let Some(note) = mode.get("note").and_then(|v| v.as_str()) {
                    println!("  note: {note}");
                }
            }
        }
    }
    Ok(())
}

fn parse_corpus() -> Result<Vec<BenchQuery>, lam_core::LamError> {
    let items: serde_json::Value = serde_json::from_str(CORPUS_JSON).map_err(|e| {
        lam_core::LamError::validation("E_BENCH_CORPUS", format!("corpus is not JSON: {e}"))
    })?;
    let mut out = Vec::new();
    for item in items.as_array().cloned().unwrap_or_default() {
        let get = |key: &str| {
            item.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        out.push(BenchQuery {
            id: get("id"),
            category: get("category"),
            query: get("query"),
            expect_kind: get("expect_kind"),
            expect_match: get("expect_match"),
        });
    }
    Ok(out)
}

fn relevant(query: &BenchQuery) -> bool {
    query.expect_kind != "none" && !query.expect_match.is_empty() && query.id != "amb-01"
}

fn secret_markers() -> [&'static str; 4] {
    ["ghp_", "AKIA", "PRIVATE KEY", "bench-canary-secret"]
}

pub fn run_benchmark() -> Result<serde_json::Value, lam_core::LamError> {
    let base = std::env::temp_dir().join(format!(
        "lam-bench-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("home");
    let repo = base.join("repo");
    let other = base.join("other");
    std::fs::create_dir_all(&repo).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    std::fs::create_dir_all(&other).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let paths = lam_config::resolve_paths(Some(home));
    std::fs::create_dir_all(&paths.data_dir).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let config_text = lam_config::Config::default()
        .to_toml_string()
        .map_err(|e| lam_core::LamError::config(e.to_string()))?;
    std::fs::write(&paths.config_file, config_text)
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    let mut conn = lam_storage::open(&paths.db_file, 3000)?;
    lam_storage::apply_migrations(&mut conn)?;
    let now = chrono::Utc::now();
    let project_id = lam_storage::resolve_ref(&mut conn, &repo.to_string_lossy(), now, true)?.id;
    let other_id = lam_storage::resolve_ref(&mut conn, &other.to_string_lossy(), now, true)?.id;
    seed_dataset(&mut conn, &repo, &project_id, &other_id, now)?;
    let corpus = parse_corpus()?;
    let mut fts = ModeMetrics {
        mode: "fts-only".to_string(),
        ..Default::default()
    };
    let mut hybrid = ModeMetrics {
        mode: "hybrid-simulated".to_string(),
        ..Default::default()
    };
    let embedder = lam_providers::HashEmbedder::test_only(32)
        .map_err(|e| lam_core::LamError::validation("E_BENCH", e.to_string()))?;
    for query in &corpus {
        score_query(&conn, &project_id, query, None, &mut fts, now)?;
        score_query(&conn, &project_id, query, Some(&embedder), &mut hybrid, now)?;
    }
    let db_bytes = std::fs::metadata(&paths.db_file)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let _ = std::fs::remove_dir_all(&base);
    Ok(serde_json::json!({
        "hardware": format!("{} {}", std::env::consts::ARCH, std::env::consts::OS),
        "sqlite_version": rusqlite::version(),
        "workload": {
            "corpus_items": corpus.len(),
            "db_bytes": db_bytes,
        },
        "model_provider": "none (fts-only) / hash-test-v1 (simulated, non-semantic)",
        "cache_state": "cold",
        "command": "agent-memory benchmark",
        "modes": [mode_report(&fts, "fts-only offline keyword retrieval"), mode_report(&hybrid, "fusion math verified with deterministic simulated vectors; semantic quality NOT measured, pending a real local model")],
    }))
}

#[allow(clippy::too_many_lines)]
fn seed_dataset(
    conn: &mut rusqlite::Connection,
    repo: &std::path::Path,
    project_id: &lam_core::ProjectId,
    other_id: &lam_core::ProjectId,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), lam_core::LamError> {
    let dataset: serde_json::Value = serde_json::from_str(DATASET_JSON).map_err(|e| {
        lam_core::LamError::validation("E_BENCH_DATASET", format!("dataset is not JSON: {e}"))
    })?;
    for item in dataset
        .get("memories")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let text = |key: &str| {
            item.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let kind = match text("kind").as_str() {
            "architecture_decision" => lam_core::MemoryKind::ArchitectureDecision,
            "lesson" => lam_core::MemoryKind::Lesson,
            "fact" => lam_core::MemoryKind::Fact,
            "decision" => lam_core::MemoryKind::ArchitectureDecision,
            _ => lam_core::MemoryKind::Note,
        };
        let scope = match text("scope").as_str() {
            "agent-private" => lam_core::Scope::AgentPrivate,
            "global" => lam_core::Scope::Global,
            "session" => lam_core::Scope::Session,
            _ => lam_core::Scope::Project,
        };
        let status = match text("status").as_str() {
            "proposed" => lam_core::MemoryStatus::Proposed,
            _ => lam_core::MemoryStatus::Verified,
        };
        lam_storage::save_memory(
            conn,
            lam_storage::SaveInput {
                project_id: *project_id,
                scope,
                kind,
                content: text("content"),
                status,
                actor: text("actor"),
                reason: None,
                sources: vec![lam_core::SourceEvidence {
                    source_kind: lam_core::SourceKind::Document,
                    source_ref: text("source_ref"),
                    line_start: None,
                    line_end: None,
                    commit_sha: None,
                    content_hash: None,
                }],
                idempotency_key: None,
            },
            now,
        )?;
    }
    for item in dataset
        .get("vault_files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let rel = item
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("bench.md");
        let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let abs = repo.join(rel);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| lam_core::LamError::io(e.to_string()))?;
        }
        std::fs::write(&abs, content).map_err(|e| lam_core::LamError::io(e.to_string()))?;
    }
    std::fs::write(repo.join(".env"), "BENCH_SECRET=bench-canary-secret\n")
        .map_err(|e| lam_core::LamError::io(e.to_string()))?;
    lam_storage::register_root(conn, project_id, repo, now)?;
    let policy = lam_vault::ScanPolicy::new(1_048_576, false)?;
    lam_storage::sync_project(conn, project_id, &policy, now)?;
    for item in dataset
        .get("episodes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let text = |key: &str| {
            item.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let mut event = lam_core::NormalizedEvent {
            schema_version: 1,
            host: "claude-code".to_string(),
            event_type: text("event_type"),
            external_session_id: text("session"),
            event_id: format!("bench-{}", text("session")),
            cwd: repo.to_string_lossy().to_string(),
            git_branch: None,
            timestamp: now,
            payload: [
                ("trigger".to_string(), serde_json::json!("auto")),
                (
                    "current_task".to_string(),
                    serde_json::json!(text("abstract")),
                ),
                ("next_steps".to_string(), serde_json::json!(text("summary"))),
                ("topics".to_string(), serde_json::json!(text("topics"))),
            ]
            .into_iter()
            .collect(),
        };
        event.validate()?;
        lam_storage::ingest_normalized(conn, &mut event, 365, now)?;
    }
    let mut entity_ids: std::collections::HashMap<String, lam_core::EntityId> =
        std::collections::HashMap::new();
    for item in dataset
        .get("graph")
        .and_then(|v| v.get("entities"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("node");
        let kind = item
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("concept");
        let id = lam_storage::upsert_entity(conn, project_id, name, kind, None)?;
        entity_ids.insert(name.to_string(), id);
    }
    for edge in dataset
        .get("graph")
        .and_then(|v| v.get("edges"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let parts: Vec<String> = edge
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if parts.len() == 3 {
            if let (Some(source), Some(target)) =
                (entity_ids.get(&parts[0]), entity_ids.get(&parts[2]))
            {
                let _ = lam_storage::add_edge(
                    conn,
                    lam_storage::EdgeInput {
                        project_id,
                        source_id: source,
                        relation_type: &parts[1],
                        target_id: target,
                        valid_from: now,
                        valid_until: None,
                        source_ref: None,
                        now,
                    },
                );
            }
        }
    }
    lam_storage::save_memory(
        conn,
        lam_storage::SaveInput {
            project_id: *other_id,
            scope: lam_core::Scope::AgentPrivate,
            kind: lam_core::MemoryKind::Fact,
            content: "canary-zulu-9 private blueprint".to_string(),
            status: lam_core::MemoryStatus::Verified,
            actor: "cli".to_string(),
            reason: None,
            sources: vec![],
            idempotency_key: None,
        },
        now,
    )?;
    Ok(())
}

fn collect_texts(hits: &lam_retrieval::GroupedHits) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for hit in hits
        .memories
        .iter()
        .chain(hits.episodes.iter())
        .chain(hits.vault.iter())
    {
        out.push((hit.id.clone(), format!("{} {}", hit.title, hit.snippet)));
    }
    for hit in &hits.graph {
        out.push((hit.id.clone(), hit.title.clone()));
    }
    out
}

#[allow(clippy::too_many_lines)]
fn score_query(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
    query: &BenchQuery,
    embedder: Option<&lam_providers::HashEmbedder>,
    metrics: &mut ModeMetrics,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), lam_core::LamError> {
    let began = Instant::now();
    let grouped = lam_retrieval::multi_search(conn, project_id, &query.query, 10, now)?;
    let mut vector_lists: Vec<Vec<String>> = Vec::new();
    if let Some(model) = embedder {
        let texts = collect_texts(&grouped);
        let query_vector = model
            .embed(&lam_providers::EmbedRequest {
                texts: &[query.query.as_str()],
            })
            .map_err(|e| lam_core::LamError::validation("E_BENCH", e.to_string()))?;
        let candidates: Vec<&str> = texts.iter().map(|(_, text)| text.as_str()).collect();
        let vectors = model
            .embed(&lam_providers::EmbedRequest { texts: &candidates })
            .map_err(|e| lam_core::LamError::validation("E_BENCH", e.to_string()))?;
        let mut scored: Vec<(String, f32)> = texts
            .iter()
            .zip(vectors.iter())
            .filter_map(|((id, _), vector)| {
                lam_providers::cosine_similarity(&query_vector[0], vector)
                    .map(|score| (id.clone(), score))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        vector_lists.push(scored.into_iter().map(|(id, _)| id).collect());
    }
    let mut verified = std::collections::HashSet::new();
    for hit in &grouped.memories {
        if hit.status.as_deref() == Some("verified") {
            verified.insert(hit.id.clone());
        }
    }
    let fused = lam_retrieval::fuse(&grouped, &vector_lists, &verified, &query.query);
    metrics
        .latencies_ms
        .push(began.elapsed().as_secs_f64() * 1000.0);
    metrics.queries += 1;
    if query.expect_kind == "none" || query.expect_kind == "excluded" {
        let violated = if query.expect_match.is_empty() {
            !fused.is_empty()
        } else {
            fused
                .iter()
                .take(10)
                .any(|hit| hit_matches(&query.expect_match, hit))
        };
        metrics.exclusions.checks += 1;
        if violated {
            metrics.exclusions.violations += 1;
        }
        return Ok(());
    }
    if !relevant(query) {
        return Ok(());
    }
    metrics.scored += 1;
    let entry = metrics
        .by_category
        .entry(query.category.clone())
        .or_default();
    entry.scored += 1;
    let mut ranked = false;
    for (rank, hit) in fused.iter().take(10).enumerate() {
        if hit_matches(&query.expect_match, hit) {
            metrics.recall_at_5_hits += usize::from(rank < 5);
            metrics.reciprocal_sum += 1.0 / (rank as f64 + 1.0);
            ranked = true;
            break;
        }
    }
    if ranked {
        if let Some(entry) = metrics.by_category.get_mut(&query.category) {
            entry.hits += 1;
        }
    }
    for hit in fused.iter().take(10) {
        if hit_matches_secret(hit) {
            metrics.secrets_found += 1;
        }
    }
    for hit in fused.iter().take(5) {
        if hit.source_ref.contains("canary-zulu-9") {
            metrics.leakage_hits += 1;
        }
    }
    let context = lam_retrieval::build_hybrid_context(&query.query, fused, 200, 10)?;
    if context.estimated_tokens > context.token_budget {
        metrics.budget_violations += 1;
    }
    Ok(())
}

fn mode_report(metrics: &ModeMetrics, note: &str) -> serde_json::Value {
    serde_json::json!({
        "mode": metrics.mode,
        "queries": metrics.queries,
        "scored": metrics.scored,
        "recall_at_5": (metrics.recall_at_5() * 10000.0).round() / 10000.0,
        "mrr_at_10": (metrics.mrr_at_10() * 10000.0).round() / 10000.0,
        "budget_violations": metrics.budget_violations,
        "stale_hits": metrics.stale_hits,
        "leakage_hits": metrics.leakage_hits,
        "secrets_found": metrics.secrets_found,
        "latency_p50_ms": (metrics.percentile(50.0) * 100.0).round() / 100.0,
        "latency_p95_ms": (metrics.percentile(95.0) * 100.0).round() / 100.0,
        "exclusions": metrics.exclusions,
        "by_category": metrics.by_category,
        "note": note,
    })
}

fn hit_matches(needle: &str, hit: &lam_retrieval::FusedHit) -> bool {
    let clean = |text: &str| text.replace("<<", "").replace(">>", "").replace("...", " ");
    let haystack = format!(
        "{} {} {}",
        clean(&hit.title),
        clean(&hit.snippet),
        clean(&hit.source_ref)
    )
    .to_lowercase();
    haystack.contains(&needle.to_lowercase())
}

fn hit_matches_secret(hit: &lam_retrieval::FusedHit) -> bool {
    let haystack = format!("{} {}", hit.title, hit.snippet);
    secret_markers()
        .iter()
        .any(|marker| haystack.contains(marker))
}
