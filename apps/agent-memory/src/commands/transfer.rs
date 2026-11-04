use clap::Args;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct ExportArgs {
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long, default_value = "jsonl")]
    pub format: String,
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    pub file: PathBuf,
    #[arg(long)]
    pub project: Option<String>,
}

pub fn run_export(
    home: Option<PathBuf>,
    args: ExportArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let format = args.format.trim().to_lowercase();
    if format != "jsonl" && format != "markdown" {
        return Err(lam_core::LamError::validation(
            "E_EXPORT_FORMAT_INVALID",
            "format must be jsonl or markdown".to_string(),
        ));
    }
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let project = match args.project.as_deref() {
        Some(reference) => Some(lam_storage::resolve_ref(&mut conn, reference, now, false)?.id),
        None => None,
    };
    if format == "jsonl" {
        let (lines, redacted) = lam_storage::export_jsonl(&conn, project.as_ref())?;
        let mut out = String::new();
        for line in &lines {
            out.push_str(line);
            out.push('\n');
        }
        if let Some(parent) = args.output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| lam_core::LamError::io(e.to_string()))?;
            }
        }
        std::fs::write(&args.output, out).map_err(|e| lam_core::LamError::io(e.to_string()))?;
        print_report(
            as_json,
            &serde_json::json!({
                "format": "jsonl",
                "output": args.output.to_string_lossy(),
                "records": lines.len(),
                "redacted": redacted,
            }),
        );
    } else {
        let document = export_markdown(&conn, project.as_ref())?;
        if let Some(parent) = args.output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| lam_core::LamError::io(e.to_string()))?;
            }
        }
        std::fs::write(&args.output, &document)
            .map_err(|e| lam_core::LamError::io(e.to_string()))?;
        print_report(
            as_json,
            &serde_json::json!({
                "format": "markdown",
                "output": args.output.to_string_lossy(),
            }),
        );
    }
    Ok(())
}

pub fn run_import(
    home: Option<PathBuf>,
    args: ImportArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let raw = std::fs::read_to_string(&args.file)
        .map_err(|e| lam_core::LamError::io(format!("cannot read {}: {e}", args.file.display())))?;
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    let now = chrono::Utc::now();
    let first = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first.starts_with('{') {
        let lines: Vec<String> = raw.lines().map(|line| line.to_string()).collect();
        let report = lam_storage::import_jsonl(&mut conn, &lines, now)?;
        print_report(
            as_json,
            &serde_json::json!({
                "format": "jsonl",
                "projects": report.projects,
                "memories": report.memories,
                "revisions": report.revisions,
                "sources": report.sources,
                "facts": report.facts,
                "skipped": report.skipped,
                "redacted": report.redacted,
            }),
        );
        return Ok(());
    }
    let reference = args.project.as_deref().ok_or_else(|| {
        lam_core::LamError::validation(
            "E_IMPORT_PROJECT_REQUIRED",
            "markdown import needs --project to bind the new memories".to_string(),
        )
    })?;
    let project = lam_storage::resolve_ref(&mut conn, reference, now, true)?.id;
    let mut imported = 0;
    for section in parse_markdown(&raw)? {
        let kind: lam_core::MemoryKind = section.kind.parse().map_err(|_| {
            lam_core::LamError::validation(
                "E_IMPORT_INVALID",
                format!("unknown kind {}", section.kind),
            )
        })?;
        let status: lam_core::MemoryStatus = section.status.parse().map_err(|_| {
            lam_core::LamError::validation(
                "E_IMPORT_INVALID",
                format!("unknown status {}", section.status),
            )
        })?;
        let scope: lam_core::Scope = section.scope.parse().map_err(|_| {
            lam_core::LamError::validation(
                "E_IMPORT_INVALID",
                format!("unknown scope {}", section.scope),
            )
        })?;
        let mut sources = Vec::new();
        for entry in &section.sources {
            sources.push(parse_source_entry(entry)?);
        }
        lam_storage::save_memory(
            &mut conn,
            lam_storage::SaveInput {
                project_id: project,
                scope,
                kind,
                content: section.content.clone(),
                status,
                actor: section.actor.clone(),
                reason: section.reason.clone(),
                sources,
                idempotency_key: None,
            },
            now,
        )?;
        imported += 1;
    }
    print_report(
        as_json,
        &serde_json::json!({
            "format": "markdown",
            "memories": imported,
        }),
    );
    Ok(())
}

fn print_report(as_json: bool, value: &serde_json::Value) {
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
        );
    } else if let Some(object) = value.as_object() {
        for (key, item) in object {
            println!("{key}: {item}");
        }
    }
}

fn export_markdown(
    conn: &rusqlite::Connection,
    project: Option<&lam_core::ProjectId>,
) -> Result<String, lam_core::LamError> {
    let mut out = String::from("# LAM export schema_version=1 format=markdown\n");
    let mut stmt_text = "SELECT id FROM memories".to_string();
    if project.is_some() {
        stmt_text.push_str(" WHERE project_id = ?1");
    }
    stmt_text.push_str(" ORDER BY rowid ASC");
    let mut stmt = conn
        .prepare(&stmt_text)
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let ids: Vec<String> = match project {
        Some(id) => stmt
            .query_map(rusqlite::params![id.to_string_value()], |row| row.get(0))
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?,
        None => stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?,
    };
    for id in &ids {
        let memory_id: lam_core::MemoryId = id
            .parse()
            .map_err(|_| lam_core::LamError::storage("stored memory id invalid".to_string()))?;
        let detail = lam_storage::get_memory(conn, &memory_id)?;
        out.push_str(&format!("\n## memory {}\n", detail.memory.id));
        out.push_str(&format!("kind: {}\n", detail.memory.kind.as_str()));
        out.push_str(&format!("status: {}\n", detail.revision.status.as_str()));
        out.push_str(&format!("scope: {}\n", detail.memory.scope.as_str()));
        out.push_str(&format!("actor: {}\n", detail.revision.actor));
        if let Some(reason) = detail.revision.reason.as_deref() {
            if !reason.trim().is_empty() {
                out.push_str(&format!("reason: {}\n", single_line(reason)));
            }
        }
        if !detail.sources.is_empty() {
            out.push_str("sources:\n");
            for source in &detail.sources {
                out.push_str(&format!("- {}\n", format_source(source)));
            }
        }
        out.push('\n');
        out.push_str(&detail.revision.content);
        if !detail.revision.content.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_source(source: &lam_core::SourceEvidence) -> String {
    let mut text = format!(
        "{}:{}",
        source_kind_text(&source.source_kind),
        source.source_ref
    );
    match (source.line_start, source.line_end) {
        (Some(start), Some(end)) => text.push_str(&format!(" lines {start}-{end}")),
        (Some(start), None) => text.push_str(&format!(" lines {start}-")),
        _ => {}
    }
    text
}

struct MarkdownSection {
    kind: String,
    status: String,
    scope: String,
    actor: String,
    reason: Option<String>,
    sources: Vec<String>,
    content: String,
}

fn parse_markdown(raw: &str) -> Result<Vec<MarkdownSection>, lam_core::LamError> {
    let invalid = |message: String| lam_core::LamError::validation("E_IMPORT_INVALID", message);
    let mut sections = Vec::new();
    let mut current: Option<MarkdownSection> = None;
    let mut body: Vec<String> = Vec::new();
    let mut in_body = false;
    let mut in_sources = false;
    for line in raw.lines() {
        if let Some(id) = line.strip_prefix("## memory ") {
            if id.trim().is_empty() {
                return Err(invalid("markdown section misses memory id".to_string()));
            }
            if let Some(mut done) = current.take() {
                done.content = body.join("\n").trim().to_string();
                sections.push(done);
                body.clear();
            }
            current = Some(MarkdownSection {
                kind: String::new(),
                status: String::new(),
                scope: "project".to_string(),
                actor: "import".to_string(),
                reason: None,
                sources: Vec::new(),
                content: String::new(),
            });
            in_body = false;
            in_sources = false;
            continue;
        }
        let Some(section) = current.as_mut() else {
            continue;
        };
        if line.trim().is_empty() {
            if in_sources || !section.kind.is_empty() {
                in_body = true;
            }
            continue;
        }
        if in_body {
            body.push(line.to_string());
            continue;
        }
        if line == "sources:" {
            in_sources = true;
            continue;
        }
        if in_sources {
            if let Some(entry) = line.strip_prefix("- ") {
                section.sources.push(entry.trim().to_string());
                continue;
            }
            in_sources = false;
        }
        if let Some((key, value)) = line.split_once(':') {
            match key.trim() {
                "kind" => section.kind = value.trim().to_string(),
                "status" => section.status = value.trim().to_string(),
                "scope" => section.scope = value.trim().to_string(),
                "actor" => section.actor = value.trim().to_string(),
                "reason" => section.reason = Some(value.trim().to_string()),
                _ => {}
            }
            continue;
        }
        in_body = true;
        body.push(line.to_string());
    }
    if let Some(mut done) = current.take() {
        done.content = body.join("\n").trim().to_string();
        sections.push(done);
    }
    for section in &sections {
        if section.kind.is_empty() || section.status.is_empty() || section.content.is_empty() {
            return Err(invalid(
                "markdown section needs kind, status and content".to_string(),
            ));
        }
    }
    if sections.is_empty() {
        return Err(invalid("no memory sections found".to_string()));
    }
    Ok(sections)
}

fn parse_source_entry(entry: &str) -> Result<lam_core::SourceEvidence, lam_core::LamError> {
    let invalid =
        || lam_core::LamError::validation("E_IMPORT_INVALID", format!("bad source entry {entry}"));
    let (head, lines) = match entry.split_once(" lines ") {
        Some((head, tail)) => (head, Some(tail)),
        None => (entry, None),
    };
    let (kind_text, reference) = head.split_once(':').ok_or_else(invalid)?;
    let source_kind = parse_source_kind(kind_text).ok_or_else(invalid)?;
    if reference.trim().is_empty() {
        return Err(invalid());
    }
    let (line_start, line_end) = match lines {
        Some(range) => {
            let range = range.trim().trim_end_matches('-');
            match range.split_once('-') {
                Some((start, end)) => (
                    Some(start.trim().parse::<u32>().map_err(|_| invalid())?),
                    Some(end.trim().parse::<u32>().map_err(|_| invalid())?),
                ),
                None => (Some(range.parse::<u32>().map_err(|_| invalid())?), None),
            }
        }
        None => (None, None),
    };
    Ok(lam_core::SourceEvidence {
        source_kind,
        source_ref: reference.trim().to_string(),
        line_start,
        line_end,
        commit_sha: None,
        content_hash: None,
    })
}

fn source_kind_text(kind: &lam_core::SourceKind) -> &'static str {
    match kind {
        lam_core::SourceKind::Document => "document",
        lam_core::SourceKind::Session => "session",
        lam_core::SourceKind::Episode => "episode",
        lam_core::SourceKind::User => "user",
        lam_core::SourceKind::Config => "config",
        lam_core::SourceKind::Test => "test",
    }
}

fn parse_source_kind(text: &str) -> Option<lam_core::SourceKind> {
    match text.trim() {
        "document" => Some(lam_core::SourceKind::Document),
        "session" => Some(lam_core::SourceKind::Session),
        "episode" => Some(lam_core::SourceKind::Episode),
        "user" => Some(lam_core::SourceKind::User),
        "config" => Some(lam_core::SourceKind::Config),
        "test" => Some(lam_core::SourceKind::Test),
        _ => None,
    }
}
