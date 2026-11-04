use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::ids::MemoryId;
use crate::ids::ProjectId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextItem {
    pub memory_id: MemoryId,
    pub kind: String,
    pub status: String,
    pub source: String,
    pub relevance: f64,
    pub valid_from: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextResponse {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub task: String,
    pub token_budget: u32,
    pub estimated_tokens: u32,
    pub context: String,
    pub items: Vec<ContextItem>,
    pub warnings: Vec<String>,
}

pub fn estimate_tokens(text: &str) -> u32 {
    std::cmp::max(1, (text.len() / 4) as u32)
}

pub fn relevance_from_rank(rank: f64) -> f64 {
    let clamped = 1.0 / (1.0 + rank.abs());
    (clamped * 10000.0).round() / 10000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_estimate_scales_with_length() {
        assert_eq!(estimate_tokens(""), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }
    #[test]
    fn relevance_stays_in_unit_range() {
        for rank in [-100.0, -1.0, -0.000001, 0.0] {
            let relevance = relevance_from_rank(rank);
            assert!((0.0..=1.0).contains(&relevance));
        }
        assert!(relevance_from_rank(-0.000001) > relevance_from_rank(-100.0));
    }
}
