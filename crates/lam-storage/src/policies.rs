#[derive(Debug, Clone, PartialEq)]
pub struct ProjectPolicy {
    pub project_id: lam_core::ProjectId,
    pub capture_enabled: bool,
    pub deny_globs: Vec<String>,
    pub sensitivity: String,
}

impl ProjectPolicy {
    pub fn default_for(project_id: lam_core::ProjectId) -> Self {
        Self {
            project_id,
            capture_enabled: true,
            deny_globs: Vec::new(),
            sensitivity: "standard".to_string(),
        }
    }

    pub fn denied(&self, path: &str) -> bool {
        self.deny_globs.iter().any(|glob| glob_match(glob, path))
    }

    pub fn strict(&self) -> bool {
        self.sensitivity.trim().to_lowercase() == "strict"
    }
}

pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim().trim_start_matches('/');
    let path = path.trim().trim_start_matches('/');
    if pattern.is_empty() || path.is_empty() {
        return false;
    }
    if match_here(pattern.as_bytes(), path.as_bytes()) {
        return true;
    }
    if let Some(base) = path.rsplit('/').next() {
        if !base.contains('/') && match_here(pattern.as_bytes(), base.as_bytes()) {
            return true;
        }
    }
    false
}

fn match_here(pattern: &[u8], text: &[u8]) -> bool {
    let (mut px, mut tx) = (0usize, 0usize);
    let (mut star, mut match_at) = (None, 0usize);
    while tx < text.len() {
        if px < pattern.len() && (pattern[px] == b'?' || pattern[px] == text[tx]) {
            px += 1;
            tx += 1;
        } else if px < pattern.len() && pattern[px] == b'*' {
            star = Some(px);
            match_at = tx;
            px += 1;
        } else if let Some(star_at) = star {
            px = star_at + 1;
            match_at += 1;
            tx = match_at;
        } else {
            return false;
        }
    }
    while px < pattern.len() && pattern[px] == b'*' {
        px += 1;
    }
    px == pattern.len()
}

pub fn get_policy(
    conn: &rusqlite::Connection,
    project_id: &lam_core::ProjectId,
) -> Result<ProjectPolicy, lam_core::LamError> {
    let row: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT capture_enabled, deny_globs, sensitivity FROM project_policies WHERE project_id = ?1",
            rusqlite::params![project_id.to_string_value()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    match row {
        None => Ok(ProjectPolicy::default_for(*project_id)),
        Some((capture, globs_json, sensitivity)) => {
            let deny_globs: Vec<String> = serde_json::from_str(&globs_json).unwrap_or_default();
            Ok(ProjectPolicy {
                project_id: *project_id,
                capture_enabled: capture != 0,
                deny_globs,
                sensitivity,
            })
        }
    }
}

pub fn set_policy(
    conn: &rusqlite::Connection,
    policy: &ProjectPolicy,
) -> Result<(), lam_core::LamError> {
    if policy.sensitivity.trim().to_lowercase() != "standard"
        && policy.sensitivity.trim().to_lowercase() != "strict"
    {
        return Err(lam_core::LamError::validation(
            "E_POLICY_SENSITIVITY_INVALID",
            "sensitivity must be standard or strict".to_string(),
        ));
    }
    for glob in &policy.deny_globs {
        if glob.trim().is_empty() || glob.len() > 256 {
            return Err(lam_core::LamError::validation(
                "E_POLICY_GLOB_INVALID",
                "deny globs must be non-empty and short".to_string(),
            ));
        }
    }
    let globs_json = serde_json::to_string(&policy.deny_globs).map_err(lam_core::LamError::from)?;
    conn.execute(
        "INSERT INTO project_policies (project_id, capture_enabled, deny_globs, sensitivity)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(project_id) DO UPDATE SET capture_enabled = excluded.capture_enabled,
             deny_globs = excluded.deny_globs, sensitivity = excluded.sensitivity",
        rusqlite::params![
            policy.project_id.to_string_value(),
            i64::from(policy.capture_enabled),
            globs_json,
            policy.sensitivity.trim().to_lowercase(),
        ],
    )
    .map_err(|e| lam_core::LamError::storage(e.to_string()))?;
    crate::db::harden_current_db(conn);
    Ok(())
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matching_covers_roots_and_names() {
        assert!(glob_match("*.env", ".env"));
        assert!(glob_match("*.env", "config/.env"));
        assert!(glob_match("secrets/*", "secrets/token.txt"));
        assert!(glob_match("id_rsa", "/home/user/.ssh/id_rsa"));
        assert!(!glob_match("*.pem", "notes.md"));
        assert!(!glob_match("", "notes.md"));
    }

    #[test]
    fn default_policy_permits_everything() {
        let id = lam_core::ProjectId::new();
        let policy = ProjectPolicy::default_for(id);
        assert!(policy.capture_enabled);
        assert!(!policy.denied("anything.md"));
        assert!(!policy.strict());
    }

    #[test]
    fn policy_round_trips() {
        let mut conn = crate::db::open_in_memory(3000).expect("open memory");
        crate::migrations::apply_migrations(&mut conn).expect("migrate");
        let dir = tempfile::tempdir().expect("tempdir");
        let project = crate::projects::resolve_or_create(
            &mut conn,
            dir.path(),
            Some("policy-proj"),
            chrono::Utc::now(),
        )
        .expect("project")
        .id;
        set_policy(
            &conn,
            &ProjectPolicy {
                project_id: project,
                capture_enabled: false,
                deny_globs: vec!["*.env".to_string()],
                sensitivity: "strict".to_string(),
            },
        )
        .expect("set");
        let loaded = get_policy(&conn, &project).expect("get");
        assert!(!loaded.capture_enabled);
        assert!(loaded.denied("config/.env"));
        assert!(loaded.strict());
    }
}
