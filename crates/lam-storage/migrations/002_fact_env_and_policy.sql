ALTER TABLE fact_assertions ADD COLUMN env TEXT NOT NULL DEFAULT '';
CREATE TABLE IF NOT EXISTS predicate_policy (
    predicate TEXT PRIMARY KEY,
    cardinality TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
INSERT OR IGNORE INTO predicate_policy (predicate, cardinality, updated_at) VALUES
    ('depends_on', 'multi', '2026-11-02T00:00:00Z'),
    ('relates_to', 'multi', '2026-11-02T00:00:00Z');
