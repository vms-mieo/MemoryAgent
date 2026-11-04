use chrono::DateTime;
use chrono::Utc;

#[derive(Debug, Clone, PartialEq)]
pub struct AdmissionDecision {
    pub action: String,
    pub reason_code: String,
    pub confidence: f64,
    pub mode: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Default)]
pub struct WriteOptions {
    pub explicit_save: bool,
    pub verified_adr: bool,
    pub secret_deny: bool,
    pub minimum_confidence: f64,
    pub model_available: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ScoredItem {
    pub index: usize,
    pub score: f64,
}

fn high_value_hits(text: &str) -> usize {
    let lower = text.to_lowercase();
    let markers = [
        "decision",
        "adr",
        "constraint",
        "root cause",
        "migration",
        "workaround",
        "fix",
        "incident",
        "adr:",
        "decided",
        "lesson",
        "convention",
        "must",
        "never",
        "always",
        "requires",
        "depends on",
        "uses",
        "timeout",
        "failure",
        "error",
        "verified",
    ];
    markers
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count()
}

fn low_value_only(text: &str) -> bool {
    let trimmed = text.trim().to_lowercase();
    if trimmed.len() < 24 {
        return true;
    }
    let fillers = [
        "hello",
        "hi there",
        "thanks",
        "thank you",
        "ok",
        "okay",
        "lgtm",
        "acknowledged",
        "noted",
        "got it",
    ];
    if fillers
        .iter()
        .any(|filler| trimmed == *filler || trimmed.starts_with(&format!("{filler} ")))
    {
        return true;
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() <= 3 {
        return true;
    }
    false
}

pub fn score_content(text: &str) -> (f64, &'static str) {
    if text.trim().is_empty() {
        return (0.05, "empty_content");
    }
    if low_value_only(text) {
        return (0.15, "low_value_filler");
    }
    let hits = high_value_hits(text);
    if hits >= 3 {
        return (0.9, "high_value_markers");
    }
    if hits >= 1 {
        return (0.65, "some_signal");
    }
    let words = text.split_whitespace().count();
    if words >= 20 {
        return (0.5, "substantive_length");
    }
    (0.35, "weak_signal")
}

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

pub fn decide_write(mode: &str, text: &str, opts: &WriteOptions) -> AdmissionDecision {
    let mode_name = mode.trim().to_lowercase();
    if opts.secret_deny {
        return AdmissionDecision {
            action: "reject".to_string(),
            reason_code: "privacy_security_deny".to_string(),
            confidence: 0.99,
            mode: mode_name,
            evidence: "fallback:deterministic secret screen".to_string(),
        };
    }
    if mode_name == "off" {
        return AdmissionDecision {
            action: "accept".to_string(),
            reason_code: "baseline_only".to_string(),
            confidence: 0.5,
            mode: mode_name,
            evidence: "fallback:deterministic baseline triage".to_string(),
        };
    }
    let (score, rationale) = score_content(text);
    let evidence = if opts.model_available {
        format!("classifier:{rationale}")
    } else {
        format!("fallback:deterministic {rationale}")
    };
    if mode_name == "shadow" {
        return AdmissionDecision {
            action: "accept".to_string(),
            reason_code: format!("shadow_{rationale}"),
            confidence: clamp01(score),
            mode: mode_name,
            evidence,
        };
    }
    if opts.explicit_save || opts.verified_adr {
        return AdmissionDecision {
            action: "accept".to_string(),
            reason_code: "explicit_save_protected".to_string(),
            confidence: clamp01(score),
            mode: mode_name,
            evidence,
        };
    }
    let threshold = opts.minimum_confidence.clamp(0.0, 1.0);
    if score < threshold {
        return AdmissionDecision {
            action: "reject".to_string(),
            reason_code: format!("gate_{rationale}"),
            confidence: clamp01(threshold - score + 0.5),
            mode: mode_name,
            evidence,
        };
    }
    if score < threshold + 0.1 {
        return AdmissionDecision {
            action: "review".to_string(),
            reason_code: format!("gate_borderline_{rationale}"),
            confidence: clamp01(score),
            mode: mode_name,
            evidence,
        };
    }
    AdmissionDecision {
        action: "accept".to_string(),
        reason_code: format!("gate_{rationale}"),
        confidence: clamp01(score),
        mode: mode_name,
        evidence,
    }
}

pub fn decide_read_scores(
    mode: &str,
    items: &[ScoredItem],
    minimum_confidence: f64,
    model_available: bool,
    token_budget: usize,
) -> Vec<usize> {
    let _ = model_available;
    let _ = token_budget;
    if mode.trim().to_lowercase() != "gate" {
        return items.iter().map(|item| item.index).collect();
    }
    let threshold = minimum_confidence.clamp(0.0, 1.0);
    let kept: Vec<usize> = items
        .iter()
        .filter(|item| item.score >= threshold)
        .map(|item| item.index)
        .collect();
    if kept.is_empty() {
        return items.iter().map(|item| item.index).collect();
    }
    kept
}

pub fn record_trace(
    conn: &rusqlite::Connection,
    seam: &str,
    decision: &AdmissionDecision,
    chosen: bool,
    now: DateTime<Utc>,
) -> Result<String, lam_core::LamError> {
    let id = format!(
        "adm-{}",
        &blake3::hash(
            format!(
                "lam-adm:v1:{seam}:{}:{}:{now}",
                decision.reason_code,
                decision.score()
            )
            .as_bytes()
        )
        .to_hex()[..16]
    );
    conn.execute(
        "INSERT INTO admission_traces (id, seam, mode, score, rationale_code, chosen, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            id,
            seam.trim(),
            decision.mode,
            decision.confidence,
            decision.reason_code,
            if chosen { 1 } else { 0 },
            now.to_rfc3339(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    lam_storage::harden_current_db(conn);
    Ok(id)
}

impl AdmissionDecision {
    fn score(&self) -> f64 {
        self.confidence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated_conn() -> rusqlite::Connection {
        let mut conn = lam_storage::open_in_memory(3000).expect("open memory");
        lam_storage::apply_migrations(&mut conn).expect("migrate");
        conn
    }

    fn at() -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 2, 9, 0, 0)
            .single()
            .expect("fixed time")
    }

    fn gate_opts() -> WriteOptions {
        WriteOptions {
            explicit_save: false,
            verified_adr: false,
            secret_deny: false,
            minimum_confidence: 0.6,
            model_available: false,
        }
    }

    #[test]
    fn off_accepts_without_classifier() {
        let decision = decide_write("off", "hello", &gate_opts());
        assert_eq!(decision.action, "accept");
        assert_eq!(decision.reason_code, "baseline_only");
    }

    #[test]
    fn shadow_never_changes_baseline() {
        let decision = decide_write("shadow", "hello", &gate_opts());
        assert_eq!(decision.action, "accept");
        assert!(decision.reason_code.starts_with("shadow_"));
        assert!(decision.evidence.contains("fallback:deterministic"));
    }

    #[test]
    fn gate_rejects_low_value_without_model() {
        let decision = decide_write("gate", "hello", &gate_opts());
        assert_eq!(decision.action, "reject");
        assert!(decision.evidence.contains("fallback:deterministic"));
    }

    #[test]
    fn gate_accepts_high_value_without_model() {
        let text = "Decision: use Temporal for orchestration; incident root cause was udp timeout handling failure";
        let decision = decide_write("gate", text, &gate_opts());
        assert_eq!(decision.action, "accept");
    }

    #[test]
    fn gate_cannot_block_explicit_save_or_adr() {
        let mut opts = gate_opts();
        opts.explicit_save = true;
        assert_eq!(decide_write("gate", "hello", &opts).action, "accept");
        let mut adr_opts = gate_opts();
        adr_opts.verified_adr = true;
        assert_eq!(decide_write("gate", "hello", &adr_opts).action, "accept");
    }

    #[test]
    fn privacy_deny_applies_in_every_mode() {
        for mode in ["off", "shadow", "gate"] {
            let mut opts = gate_opts();
            opts.secret_deny = true;
            let decision = decide_write(mode, "Decision: rotate keys now", &opts);
            assert_eq!(decision.action, "reject");
            assert_eq!(decision.reason_code, "privacy_security_deny");
        }
    }

    #[test]
    fn read_gate_falls_back_instead_of_emptying() {
        let items = vec![
            ScoredItem {
                index: 0,
                score: 0.1,
            },
            ScoredItem {
                index: 1,
                score: 0.2,
            },
        ];
        let kept = decide_read_scores("gate", &items, 0.9, false, 200);
        assert_eq!(kept, vec![0, 1]);
        let mixed = vec![
            ScoredItem {
                index: 0,
                score: 0.1,
            },
            ScoredItem {
                index: 1,
                score: 0.95,
            },
        ];
        assert_eq!(decide_read_scores("gate", &mixed, 0.6, false, 200), vec![1]);
        let off = decide_read_scores("off", &items, 0.9, false, 200);
        assert_eq!(off, vec![0, 1]);
    }

    #[test]
    fn traces_record_redacted_rationale() {
        let conn = migrated_conn();
        let decision = decide_write("shadow", "hello", &gate_opts());
        let id = record_trace(&conn, "write", &decision, true, at()).expect("trace");
        let (rationale, score): (String, f64) = conn
            .query_row(
                "SELECT rationale_code, score FROM admission_traces WHERE id = ?1",
                rusqlite::params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("row");
        assert!(rationale.starts_with("shadow_"));
        assert!((score - decision.confidence).abs() < 1e-9);
    }
}
