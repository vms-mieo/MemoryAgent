CREATE TABLE IF NOT EXISTS dedup_suggestions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    memory_a_id TEXT NOT NULL,
    memory_b_id TEXT NOT NULL,
    score REAL NOT NULL,
    reason TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(project_id, memory_a_id, memory_b_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_dedup_status ON dedup_suggestions(project_id, status);
