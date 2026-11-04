CREATE TABLE IF NOT EXISTS remote_calls (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    endpoint_host TEXT NOT NULL,
    payload_bytes INTEGER NOT NULL,
    redactions INTEGER NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_remote_calls_created ON remote_calls(created_at);
