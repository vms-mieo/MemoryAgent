use clap::Args;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum IndexAction {
    Rebuild(IndexRebuildArgs),
}

#[derive(Debug, Args)]
pub struct IndexRebuildArgs {
    #[arg(long, default_value_t = false)]
    pub vacuum: bool,
}

pub fn run(
    home: Option<PathBuf>,
    action: IndexAction,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    match action {
        IndexAction::Rebuild(args) => run_rebuild(home, args, as_json),
    }
}

fn run_rebuild(
    home: Option<PathBuf>,
    args: IndexRebuildArgs,
    as_json: bool,
) -> Result<(), lam_core::LamError> {
    let (mut conn, _) = crate::commands::open_existing(&home)?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    let before: RebuildCounts = counts(&conn)?;
    let tx = conn
        .transaction()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute("DELETE FROM memory_fts", [])
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute(
        "INSERT INTO memory_fts (memory_id, content)
         SELECT m.id, r.content FROM memories m
         JOIN memory_revisions r ON r.id = m.current_revision",
        [],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute("DELETE FROM episode_fts", [])
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute(
        "INSERT INTO episode_fts (episode_id, abstract, summary, topics)
         SELECT id, abstract, summary, topics FROM episodes",
        [],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute("DELETE FROM document_fts", [])
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.execute(
        "INSERT INTO document_fts (document_id, chunk_id, content, title, relative_path)
         SELECT c.document_id, c.id, c.content, d.title, d.relative_path
         FROM document_chunks c JOIN documents d ON d.id = c.document_id",
        [],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    tx.commit()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    for table in ["memory_fts", "episode_fts", "document_fts"] {
        conn.execute(
            &format!("INSERT INTO {table}({table}) VALUES('optimize')"),
            [],
        )
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    }
    if args.vacuum {
        conn.execute_batch("VACUUM;")
            .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    }
    lam_storage::harden_current_db(&conn);
    lam_storage::integrity_check(&conn)?;
    let after: RebuildCounts = counts(&conn)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "memory_fts_before": before.memory_fts,
                "memory_fts_after": after.memory_fts,
                "episode_fts_before": before.episode_fts,
                "episode_fts_after": after.episode_fts,
                "document_fts_before": before.document_fts,
                "document_fts_after": after.document_fts,
                "vacuum": args.vacuum,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "rebuilt memory_fts {}->{} episode_fts {}->{} document_fts {}->{}",
            before.memory_fts,
            after.memory_fts,
            before.episode_fts,
            after.episode_fts,
            before.document_fts,
            after.document_fts
        );
    }
    Ok(())
}

#[derive(Debug, Default)]
struct RebuildCounts {
    memory_fts: i64,
    episode_fts: i64,
    document_fts: i64,
}

fn counts(conn: &rusqlite::Connection) -> Result<RebuildCounts, lam_core::LamError> {
    let one = |sql: &str| -> Result<i64, lam_core::LamError> {
        conn.query_row(sql, [], |row| row.get(0))
            .map_err(|e| lam_core::LamError::storage(e.to_string()))
    };
    Ok(RebuildCounts {
        memory_fts: one("SELECT COUNT(*) FROM memory_fts")?,
        episode_fts: one("SELECT COUNT(*) FROM episode_fts")?,
        document_fts: one("SELECT COUNT(*) FROM document_fts")?,
    })
}
