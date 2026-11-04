#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    Memory,
    Episode,
    Vault,
    Graph,
}

impl HitKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            HitKind::Memory => "memory",
            HitKind::Episode => "episode",
            HitKind::Vault => "vault",
            HitKind::Graph => "entity",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnifiedHit {
    pub id: String,
    pub kind: HitKind,
    pub title: String,
    pub snippet: String,
    pub source_ref: String,
    pub rank: f64,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GroupedHits {
    pub memories: Vec<UnifiedHit>,
    pub episodes: Vec<UnifiedHit>,
    pub vault: Vec<UnifiedHit>,
    pub graph: Vec<UnifiedHit>,
}

impl GroupedHits {
    pub fn total(&self) -> usize {
        self.memories.len() + self.episodes.len() + self.vault.len() + self.graph.len()
    }
}
