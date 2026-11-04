use std::path::PathBuf;
use std::process::Command;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agent-memory"))
}

fn fresh_home(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "lam-phase0-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    base
}

fn run_init(home: &std::path::Path, extra: &[&str]) -> std::process::Output {
    let mut args: Vec<String> = vec![
        "--home".to_string(),
        home.to_string_lossy().to_string(),
        "init".to_string(),
    ];
    for item in extra {
        args.push(item.to_string());
    }
    Command::new(binary())
        .args(args)
        .env("AGENT_MEMORY_HOME", home)
        .output()
        .expect("spawn agent-memory init")
}

fn schema_tables(db_file: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(db_file).expect("open db for inspection");
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','virtual') ORDER BY name")
        .expect("prepare list");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query tables");
    rows.map(|r| r.expect("row string")).collect()
}

#[test]
fn init_creates_valid_workspace_offline() {
    let home = fresh_home("offline");
    let output = run_init(&home, &[]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home.join("config.toml").exists());
    assert!(home.join("memory.db").exists());
    let conn = rusqlite::Connection::open(home.join("memory.db")).expect("open created db");
    let missing = lam_storage::check_schema_valid(&conn).expect("schema check runs");
    assert!(missing.is_empty(), "missing tables: {missing:?}");
    lam_storage::integrity_check(&conn).expect("integrity ok");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn repeated_init_leaves_config_and_data_unchanged() {
    let home = fresh_home("idempotent");
    let first = run_init(&home, &[]);
    assert!(first.status.success());
    let config_before = std::fs::read(home.join("config.toml")).expect("read config 1");
    let config_text = String::from_utf8(config_before.clone()).expect("config utf8");
    lam_config::Config::from_toml_str(&config_text).expect("config valid");
    let conn = rusqlite::Connection::open(home.join("memory.db")).expect("open db");
    let tables_before = schema_tables(&home.join("memory.db"));
    let versions_before = lam_storage::applied_versions(&conn).expect("versions");
    drop(conn);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = run_init(&home, &[]);
    assert!(
        second.status.success(),
        "second init failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let config_after = std::fs::read(home.join("config.toml")).expect("read config 2");
    assert_eq!(
        config_before, config_after,
        "config must be unchanged on repeated init"
    );
    let conn2 = rusqlite::Connection::open(home.join("memory.db")).expect("open db 2");
    let tables_after = schema_tables(&home.join("memory.db"));
    let versions_after = lam_storage::applied_versions(&conn2).expect("versions 2");
    assert_eq!(tables_before, tables_after);
    assert_eq!(versions_before, versions_after);
    lam_storage::integrity_check(&conn2).expect("integrity ok after repeat");
    let _ = std::fs::remove_dir_all(&home);
}

fn assert_healthy_db(home: &std::path::Path, versions: &[u32]) {
    let conn = rusqlite::Connection::open(home.join("memory.db")).expect("open db");
    let missing = lam_storage::check_schema_valid(&conn).expect("schema check");
    assert!(
        missing.is_empty(),
        "missing tables after concurrent init: {missing:?}"
    );
    lam_storage::integrity_check(&conn).expect("integrity ok after concurrent init");
    let applied = lam_storage::applied_versions(&conn).expect("versions");
    assert_eq!(applied, versions);
    let counts: std::collections::HashMap<u32, i64> = {
        let mut stmt = conn
            .prepare("SELECT version, COUNT(*) FROM schema_migrations GROUP BY version")
            .expect("count versions");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query versions")
            .map(|row| row.expect("version row"))
            .collect()
    };
    for version in versions {
        assert_eq!(
            counts.get(version).copied(),
            Some(1),
            "version {version} must have exactly one row"
        );
    }
}

fn run_concurrent_inits(home: &std::path::Path, count: usize) {
    let mut handles = Vec::new();
    for _ in 0..count {
        let dir = home.to_path_buf();
        handles.push(std::thread::spawn(move || run_init(&dir, &[])));
    }
    let mut ok_count = 0;
    for handle in handles {
        let output = handle.join().expect("thread join");
        assert!(
            output.status.success(),
            "concurrent init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        ok_count += 1;
    }
    assert_eq!(ok_count, count);
}

fn init_db_at_v1(home: &std::path::Path) {
    std::fs::create_dir_all(home).expect("create home");
    let first = lam_storage::MIGRATIONS
        .iter()
        .find(|m| m.version == 1)
        .expect("v1 migration");
    let db_path = home.join("memory.db");
    let conn = rusqlite::Connection::open(&db_path).expect("create db");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            checksum TEXT NOT NULL,
            applied_at TEXT NOT NULL
        ) STRICT;",
    )
    .expect("tracking table");
    conn.execute_batch(first.sql).expect("apply v1");
    let digest = blake3::hash(first.sql.as_bytes()).to_hex().to_string();
    conn.execute(
        "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?1, ?2, ?3, '2026-11-02T00:00:00Z')",
        rusqlite::params![first.version, first.name, digest],
    )
    .expect("mark v1");
}

#[test]
fn three_concurrent_inits_yield_one_valid_db() {
    for _ in 0..3 {
        let home = fresh_home("concurrent");
        run_concurrent_inits(&home, 8);
        assert_healthy_db(&home, &[1, 2, 3, 4, 5, 6]);
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[test]
fn concurrent_upgrade_from_v1_applies_once() {
    for _ in 0..3 {
        let home = fresh_home("upgrade-race");
        init_db_at_v1(&home);
        run_concurrent_inits(&home, 8);
        assert_healthy_db(&home, &[1, 2, 3, 4, 5, 6]);
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[test]
fn status_and_doctor_pass_after_init() {
    let home = fresh_home("statusdoctor");
    let init = run_init(&home, &[]);
    assert!(init.status.success());
    let status = Command::new(binary())
        .args(["--home", &home.to_string_lossy(), "status", "--json"])
        .output()
        .expect("run status");
    assert!(status.status.success());
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).expect("status json");
    assert_eq!(value.get("db").and_then(|v| v.as_str()), Some("ok"));
    let doctor = Command::new(binary())
        .args(["--home", &home.to_string_lossy(), "doctor", "--json"])
        .output()
        .expect("run doctor");
    assert!(
        doctor.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[cfg(unix)]
#[test]
fn init_sets_secure_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let home = fresh_home("perms");
    let output = run_init(&home, &[]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dir_mode = std::fs::metadata(&home)
        .expect("stat home")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "data dir must be 0700");
    let config_mode = std::fs::metadata(home.join("config.toml"))
        .expect("stat config")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(config_mode, 0o600, "config must be 0600");
    let db_mode = std::fs::metadata(home.join("memory.db"))
        .expect("stat db")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(db_mode, 0o600, "db must be 0600");
    for sidecar in ["memory.db-wal", "memory.db-shm"] {
        let path = home.join(sidecar);
        if path.exists() {
            let mode = std::fs::metadata(&path)
                .expect("stat sidecar")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{sidecar} must be 0600");
        }
    }
    let doctor = Command::new(binary())
        .args(["--home", &home.to_string_lossy(), "doctor", "--json"])
        .output()
        .expect("run doctor");
    assert!(
        doctor.status.success(),
        "doctor must pass secure perms: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&doctor.stdout).expect("doctor json");
    let checks = value
        .get("checks")
        .and_then(|v| v.as_array())
        .expect("doctor checks array");
    for wanted in ["data_dir_perms", "config_perms", "db_perms"] {
        let found = checks
            .iter()
            .find(|c| c.get("name").and_then(|v| v.as_str()) == Some(wanted));
        assert_eq!(
            found.and_then(|c| c.get("ok").and_then(|v| v.as_bool())),
            Some(true),
            "{wanted} check must pass"
        );
    }
    let _ = std::fs::remove_dir_all(&home);
}
