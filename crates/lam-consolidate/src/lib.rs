pub mod admission;
pub mod dedup;
pub mod dream;
pub mod pipeline;
pub mod runner;

pub use admission::decide_read_scores;
pub use admission::decide_write;
pub use admission::record_trace;
pub use admission::score_content;
pub use admission::AdmissionDecision;
pub use admission::ScoredItem;
pub use admission::WriteOptions;

pub use dedup::list_suggestions;
pub use dedup::resolve_suggestion;
pub use dedup::scan_project;
pub use dream::find_eligible_clusters;
pub use dream::synthesize;
pub use dream::DreamCluster;
pub use dream::DreamOutcome;

pub use pipeline::handle_episode_summarize;
pub use pipeline::handle_fact_extract;
pub use pipeline::ExtractOutcome;
pub use pipeline::ExtractorChoice;
pub use pipeline::PipelineContext;
pub use runner::maintain;
pub use runner::MaintainOutcome;
