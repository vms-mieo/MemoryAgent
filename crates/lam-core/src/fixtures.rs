use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::clock::Clock;
use crate::entities::AgentKind;
use crate::entities::FactRecord;
use crate::entities::MemoryKind;
use crate::entities::MemoryStatus;
use crate::entities::NormalizedEvent;
use crate::entities::SourceEvidence;
use crate::entities::SourceKind;
use crate::ids::MemoryId;
use crate::ids::ProjectId;
use crate::scope::Scope;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureBundle {
    pub project_id: ProjectId,
    pub project_name: String,
    pub canonical_root: String,
    pub fact_records: Vec<FactRecord>,
    pub events: Vec<NormalizedEvent>,
    pub generated_at: DateTime<Utc>,
}

fn deterministic_uuid(seed: u128, index: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(seed ^ (index.wrapping_mul(0x9E3779B97F4A7C15)))
}

pub fn generate_bundle(seed: u64, count: usize, clock: &dyn Clock) -> FixtureBundle {
    let generated_at = clock.now();
    let project_id = crate::ids::ProjectId::from_uuid(deterministic_uuid(seed as u128, 1));
    let mut fact_records = Vec::with_capacity(count);
    let mut events = Vec::with_capacity(count);
    for i in 0..count {
        let memory_id = MemoryId::from_uuid(deterministic_uuid(seed as u128, 1000 + i as u128));
        let recorded_at = generated_at + chrono::Duration::seconds(i as i64 * 60);
        fact_records.push(FactRecord {
            schema_version: 1,
            memory_id,
            project_id,
            scope: Scope::Project,
            kind: MemoryKind::Fact,
            subject: format!("fixture.subject.{i}"),
            predicate: "uses".to_string(),
            value: serde_json::json!(format!("fixture-value-{i}")),
            status: MemoryStatus::Verified,
            confidence: 0.9,
            valid_from: recorded_at,
            valid_until: None,
            recorded_at,
            source: SourceEvidence {
                source_kind: SourceKind::User,
                source_ref: format!("fixture/seed-{seed}/{i}.md"),
                line_start: Some(1),
                line_end: Some(10),
                commit_sha: None,
                content_hash: Some(
                    blake3::hash(format!("fixture-{seed}-{i}").as_bytes())
                        .to_hex()
                        .to_string(),
                ),
            },
        });
        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "summary".to_string(),
            serde_json::json!(format!("fixture event {i}")),
        );
        events.push(NormalizedEvent {
            schema_version: 1,
            host: "codex".to_string(),
            event_type: "session.end".to_string(),
            external_session_id: format!("fixture-session-{seed}"),
            event_id: format!("fixture-event-{seed}-{i}"),
            cwd: "/workspace/fixture-repo".to_string(),
            git_branch: Some("main".to_string()),
            timestamp: recorded_at,
            payload,
        });
    }
    FixtureBundle {
        project_id,
        project_name: format!("fixture-project-{seed}"),
        canonical_root: "/workspace/fixture-repo".to_string(),
        fact_records,
        events,
        generated_at,
    }
}

pub fn example_fact_record(clock: &dyn Clock) -> FactRecord {
    generate_bundle(1, 1, clock)
        .fact_records
        .into_iter()
        .next()
        .expect("bundle with one record yields a record")
}

pub fn example_event(clock: &dyn Clock) -> NormalizedEvent {
    generate_bundle(1, 1, clock)
        .events
        .into_iter()
        .next()
        .expect("bundle with one event yields an event")
}

pub fn example_agent_kinds() -> Vec<AgentKind> {
    vec![
        AgentKind::Codex,
        AgentKind::ClaudeCode,
        AgentKind::Opencode,
        AgentKind::Cli,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    #[test]
    fn bundle_is_deterministic_for_same_seed_and_time() {
        let clock_a = ManualClock::at(2026, 11, 1, 10, 0, 0);
        let clock_b = ManualClock::at(2026, 11, 1, 10, 0, 0);
        let a = generate_bundle(42, 5, &clock_a);
        let b = generate_bundle(42, 5, &clock_b);
        assert_eq!(a, b);
    }
    #[test]
    fn bundle_differs_for_different_seeds() {
        let clock = ManualClock::at(2026, 11, 1, 10, 0, 0);
        let a = generate_bundle(1, 3, &clock);
        let b = generate_bundle(2, 3, &clock);
        assert_ne!(a.fact_records, b.fact_records);
    }
    #[test]
    fn generated_records_validate() {
        let clock = ManualClock::at(2026, 11, 1, 10, 0, 0);
        let bundle = generate_bundle(7, 3, &clock);
        for record in &bundle.fact_records {
            record.validate().expect("fixture record valid");
        }
        for event in &bundle.events {
            event.validate().expect("fixture event valid");
        }
    }
}
