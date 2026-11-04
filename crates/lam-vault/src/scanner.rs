use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ScanPolicy {
    pub max_file_bytes: u64,
    pub follow_symlinks: bool,
}

impl ScanPolicy {
    pub fn new(max_file_bytes: u64, follow_symlinks: bool) -> Result<Self, lam_core::LamError> {
        if max_file_bytes == 0 {
            return Err(lam_core::LamError::validation(
                "E_VAULT_POLICY",
                "max file bytes must be greater than 0".to_string(),
            ));
        }
        Ok(Self {
            max_file_bytes,
            follow_symlinks,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub size: u64,
    pub hash: String,
    pub kind: crate::parse::FileKind,
}

fn denied_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".venv"
            | "__pycache__"
            | ".cache"
            | "vendor"
            | ".svn"
            | ".hg"
            | "out"
            | "coverage"
            | ".next"
            | ".nuxt"
    )
}

fn denied_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower == ".env" || lower.starts_with(".env.") {
        return true;
    }
    if lower == "id_rsa" || lower == "id_ed25519" {
        return true;
    }
    matches!(
        lower.rsplit('.').next(),
        Some("pem") | Some("key") | Some("sqlite") | Some("sqlite3") | Some("db")
    )
}

fn has_binary_bytes(sample: &[u8]) -> bool {
    sample.contains(&0)
}

fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let suffix = path.strip_prefix(root).ok()?;
    let mut parts: Vec<String> = Vec::new();
    for component in suffix.components() {
        match component {
            std::path::Component::Normal(text) => parts.push(text.to_string_lossy().to_string()),
            _ => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

pub fn scan_root(root: &Path, policy: &ScanPolicy) -> Result<Vec<ScannedFile>, lam_core::LamError> {
    let canonical_root = std::fs::canonicalize(root).map_err(|e| {
        lam_core::LamError::validation("E_VAULT_ROOT", format!("vault root unreadable: {}", e))
    })?;
    if !canonical_root.is_dir() {
        return Err(lam_core::LamError::validation(
            "E_VAULT_ROOT",
            "vault root is not a directory".to_string(),
        ));
    }
    let mut found: Vec<ScannedFile> = Vec::new();
    let walker = ignore::WalkBuilder::new(&canonical_root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false)
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| !denied_dir(name))
                .unwrap_or(false)
        })
        .build();
    for entry in walker {
        let entry = entry.map_err(|e| lam_core::LamError::io(e.to_string()))?;
        let file_type = entry
            .file_type()
            .ok_or_else(|| lam_core::LamError::io("unreadable entry file type".to_string()))?;
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if denied_file(&file_name) {
            continue;
        }
        if file_type.is_symlink() && !policy.follow_symlinks {
            continue;
        }
        let resolved = std::fs::canonicalize(entry.path())
            .map_err(|e| lam_core::LamError::io(e.to_string()))?;
        if !resolved.starts_with(&canonical_root) {
            continue;
        }
        let kind = match crate::parse::FileKind::of(&resolved) {
            Some(kind) => kind,
            None => continue,
        };
        let metadata =
            std::fs::metadata(&resolved).map_err(|e| lam_core::LamError::io(e.to_string()))?;
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > policy.max_file_bytes {
            continue;
        }
        let bytes = std::fs::read(&resolved).map_err(|e| lam_core::LamError::io(e.to_string()))?;
        let sample_len = bytes.len().min(8192);
        if has_binary_bytes(&bytes[..sample_len]) {
            continue;
        }
        let rel = match rel_path(&canonical_root, &resolved) {
            Some(rel) => rel,
            None => continue,
        };
        found.push(ScannedFile {
            abs_path: resolved,
            rel_path: rel,
            size: bytes.len() as u64,
            hash: crate::hash::blake3_hex(&bytes),
            kind,
        });
    }
    found.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        let mut file = std::fs::File::create(path).expect("create");
        file.write_all(bytes).expect("write");
    }

    fn policy() -> ScanPolicy {
        ScanPolicy::new(1_048_576, false).expect("policy")
    }

    #[test]
    fn allowlisted_markdown_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(&dir.path().join("docs").join("adr.md"), b"# ADR\n\nBody");
        let found = scan_root(dir.path(), &policy()).expect("scan");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rel_path, "docs/adr.md");
        assert_eq!(found[0].size, 11);
        assert_eq!(found[0].hash.len(), 64);
    }

    #[test]
    fn deny_dotenv_and_keys_and_binaries() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(&dir.path().join(".env"), b"TOKEN=abc");
        write_file(&dir.path().join("key.pem"), b"-----BEGIN PRIVATE KEY-----");
        write_file(&dir.path().join("data.sqlite"), b"sqlite?");
        write_file(&dir.path().join("notes.md"), b"# Notes");
        let mut blob = vec![0x89, b'P', b'N', b'G'];
        blob.extend_from_slice(&[0u8; 100]);
        write_file(&dir.path().join("pic.png"), &blob);
        let found = scan_root(dir.path(), &policy()).expect("scan");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rel_path, "notes.md");
    }

    #[test]
    fn symlink_escape_blocked() {
        let outside = tempfile::tempdir().expect("outside");
        write_file(&outside.path().join("secret.md"), b"# Secret outside root");
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(&dir.path().join("ok.md"), b"# Ok");
        std::os::unix::fs::symlink(outside.path().join("secret.md"), dir.path().join("evil.md"))
            .expect("symlink");
        let found = scan_root(dir.path(), &policy()).expect("scan");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rel_path, "ok.md");
        let following = ScanPolicy::new(1_048_576, true).expect("policy");
        let found_follow = scan_root(dir.path(), &following).expect("scan follow");
        assert!(found_follow.iter().all(|f| f.rel_path == "ok.md"));
    }

    #[test]
    fn denied_dirs_and_gitignore_respected() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(&dir.path().join(".gitignore"), b"*.log\n");
        write_file(&dir.path().join("debug.log"), b"logs");
        write_file(
            &dir.path().join("node_modules").join("pkg").join("index.md"),
            b"# Pkg",
        );
        write_file(&dir.path().join("keep.md"), b"# Keep");
        let found = scan_root(dir.path(), &policy()).expect("scan");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rel_path, "keep.md");
    }

    #[test]
    fn oversize_files_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_file(&dir.path().join("big.md"), &vec![b'x'; 2048]);
        let small = ScanPolicy::new(1024, false).expect("policy");
        let found = scan_root(dir.path(), &small).expect("scan");
        assert!(found.is_empty());
    }

    #[test]
    fn missing_root_rejected() {
        let result = scan_root(Path::new("/nonexistent-lam-vault-root"), &policy());
        assert!(result.is_err());
    }
}
