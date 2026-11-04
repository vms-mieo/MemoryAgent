CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    canonical_root TEXT NOT NULL,
    identity_hash TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS project_paths (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    PRIMARY KEY (project_id, path)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_project_paths_path ON project_paths(path);
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    agent_kind TEXT NOT NULL,
    external_session_id TEXT,
    branch TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT
) STRICT;
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
CREATE TABLE IF NOT EXISTS session_events (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    event_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    sanitized_payload TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_session_events_session ON session_events(session_id);
CREATE TABLE IF NOT EXISTS episodes (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    abstract TEXT NOT NULL,
    summary TEXT NOT NULL,
    topics TEXT NOT NULL,
    quality REAL NOT NULL,
    expires_at TEXT
) STRICT;
CREATE TABLE IF NOT EXISTS working_states (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    task_key TEXT NOT NULL,
    state_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    kind TEXT NOT NULL,
    current_revision TEXT,
    created_at TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_memories_project ON memories(project_id);
CREATE TABLE IF NOT EXISTS memory_revisions (
    id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    status TEXT NOT NULL,
    version INTEGER NOT NULL,
    recorded_at TEXT NOT NULL,
    actor TEXT NOT NULL,
    reason TEXT
) STRICT;
CREATE INDEX IF NOT EXISTS idx_memory_revisions_memory ON memory_revisions(memory_id);
CREATE TABLE IF NOT EXISTS memory_sources (
    id TEXT PRIMARY KEY,
    revision_id TEXT NOT NULL REFERENCES memory_revisions(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    line_start INTEGER,
    line_end INTEGER,
    commit_sha TEXT,
    content_hash TEXT
) STRICT;
CREATE INDEX IF NOT EXISTS idx_memory_sources_revision ON memory_sources(revision_id);
CREATE TABLE IF NOT EXISTS fact_assertions (
    id TEXT PRIMARY KEY,
    memory_revision_id TEXT NOT NULL REFERENCES memory_revisions(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    value_json TEXT NOT NULL,
    valid_from TEXT NOT NULL,
    valid_until TEXT,
    recorded_at TEXT NOT NULL,
    retracted_at TEXT,
    confidence REAL NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_fact_subject_predicate ON fact_assertions(subject, predicate);
CREATE TABLE IF NOT EXISTS entities (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    canonical_name TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    metadata TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_entities_project_name ON entities(project_id, canonical_name);
CREATE TABLE IF NOT EXISTS entity_aliases (
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    alias TEXT NOT NULL,
    PRIMARY KEY (entity_id, alias)
) STRICT;
CREATE TABLE IF NOT EXISTS relations (
    id TEXT PRIMARY KEY,
    source_entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    relation_type TEXT NOT NULL,
    target_entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    valid_from TEXT NOT NULL,
    valid_until TEXT,
    source_id TEXT
) STRICT;
CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_entity_id);
CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_entity_id);
CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    root_id TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    hash TEXT NOT NULL,
    title TEXT,
    indexed_at TEXT NOT NULL,
    status TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_documents_project_path ON documents(project_id, relative_path);
CREATE TABLE IF NOT EXISTS document_chunks (
    id TEXT PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    content TEXT NOT NULL,
    line_start INTEGER NOT NULL,
    line_end INTEGER NOT NULL,
    hash TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_chunks_document ON document_chunks(document_id);
CREATE TABLE IF NOT EXISTS document_links (
    source_document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    target_document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    anchor TEXT,
    link_type TEXT NOT NULL,
    PRIMARY KEY (source_document_id, target_document_id, link_type)
) STRICT;
CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(memory_id, content, tokenize='porter unicode61');
CREATE VIRTUAL TABLE IF NOT EXISTS document_fts USING fts5(document_id, chunk_id, content, title, relative_path, tokenize='porter unicode61');
CREATE TABLE IF NOT EXISTS vector_index (
    id TEXT PRIMARY KEY,
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    embedding_model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    vector BLOB,
    created_at TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_vector_owner ON vector_index(owner_kind, owner_id);
CREATE TABLE IF NOT EXISTS maintenance_jobs (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    input_ref TEXT NOT NULL,
    state TEXT NOT NULL,
    attempts INTEGER NOT NULL,
    due_at TEXT NOT NULL,
    lease_until TEXT,
    error TEXT
) STRICT;
CREATE INDEX IF NOT EXISTS idx_jobs_state_due ON maintenance_jobs(state, due_at);
CREATE TABLE IF NOT EXISTS conflict_reviews (
    id TEXT PRIMARY KEY,
    active_assertion_id TEXT NOT NULL,
    incoming_assertion_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    status TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS admission_traces (
    id TEXT PRIMARY KEY,
    seam TEXT NOT NULL,
    mode TEXT NOT NULL,
    score REAL NOT NULL,
    rationale_code TEXT NOT NULL,
    chosen INTEGER NOT NULL,
    created_at TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS audit_log (
    id TEXT PRIMARY KEY,
    event TEXT NOT NULL,
    actor TEXT NOT NULL,
    target TEXT,
    old_hash TEXT,
    new_hash TEXT,
    at TEXT NOT NULL
) STRICT;
