CREATE TABLE IF NOT EXISTS project_policies (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    capture_enabled INTEGER NOT NULL DEFAULT 1,
    deny_globs TEXT NOT NULL DEFAULT '[]',
    sensitivity TEXT NOT NULL DEFAULT 'standard'
) STRICT;
