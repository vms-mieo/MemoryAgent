CREATE TABLE IF NOT EXISTS vault_roots (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(project_id, path)
) STRICT;
CREATE VIRTUAL TABLE IF NOT EXISTS episode_fts USING fts5(episode_id, abstract, summary, topics, tokenize='porter unicode61');
INSERT INTO episode_fts (episode_id, abstract, summary, topics)
SELECT id, abstract, summary, topics FROM episodes
WHERE NOT EXISTS (SELECT 1 FROM episode_fts WHERE episode_fts.episode_id = episodes.id);
CREATE TABLE IF NOT EXISTS embedding_models (
    model_id TEXT PRIMARY KEY,
    dimensions INTEGER NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;
