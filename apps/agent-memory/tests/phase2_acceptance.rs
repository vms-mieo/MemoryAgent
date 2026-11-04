use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agent-memory"))
}

fn fresh_home(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "lam-phase2-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("temp home");
    base
}

fn run_cli(home: &Path, args: &[&str], stdin_bytes: Option<&[u8]>) -> std::process::Output {
    let mut full: Vec<String> = vec!["--home".to_string(), home.to_string_lossy().to_string()];
    for item in args {
        full.push(item.to_string());
    }
    let mut child = Command::new(binary())
        .args(full)
        .env("AGENT_MEMORY_HOME", home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn agent-memory");
    if let Some(bytes) = stdin_bytes {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(bytes)
            .expect("write stdin");
    }
    child.wait_with_output().expect("wait")
}

fn fixture(name: &str) -> Vec<u8> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("hooks")
        .join(name);
    std::fs::read(&base).unwrap_or_else(|_| panic!("missing fixture {name}"))
}

fn native_with_cwd(name: &str, cwd: &Path) -> Vec<u8> {
    let raw = fixture(name);
    let mut value: serde_json::Value = serde_json::from_slice(&raw).expect("fixture json");
    if let Some(object) = value.as_object_mut() {
        object.insert("cwd".to_string(), serde_json::json!(cwd.to_string_lossy()));
    }
    serde_json::to_vec(&value).expect("json bytes")
}

#[test]
fn claude_session_visible_from_codex_and_opencode() {
    let home = fresh_home("handoff");
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    let checkpoint = native_with_cwd("claude-precompact.json", &project_dir);
    let ingested = run_cli(
        &home,
        &[
            "hook",
            "ingest",
            "--host",
            "claude-code",
            "--event",
            "PreCompact",
        ],
        Some(&checkpoint),
    );
    assert!(
        ingested.status.success(),
        "ingest failed: {}",
        String::from_utf8_lossy(&ingested.stderr)
    );
    let codex_start = native_with_cwd("codex-sessionstart.json", &project_dir);
    let codex_context = run_cli(
        &home,
        &[
            "hook",
            "context",
            "--host",
            "codex",
            "--event",
            "SessionStart",
        ],
        Some(&codex_start),
    );
    assert!(codex_context.status.success());
    let codex_text = String::from_utf8_lossy(&codex_context.stdout).to_string();
    assert!(
        codex_text.contains("Wire worker deployment config"),
        "codex context missing checkpoint task: {codex_text}"
    );
    assert!(
        codex_text.contains("Waiting on staging credentials"),
        "codex context missing blockers: {codex_text}"
    );
    let opencode_start = native_with_cwd("opencode-session-created.json", &project_dir);
    let opencode_context = run_cli(
        &home,
        &[
            "hook",
            "context",
            "--host",
            "opencode",
            "--event",
            "session.created",
        ],
        Some(&opencode_start),
    );
    assert!(opencode_context.status.success());
    let opencode_text = String::from_utf8_lossy(&opencode_context.stdout).to_string();
    let parsed: serde_json::Value =
        serde_json::from_str(&opencode_text).expect("opencode context json");
    let body = parsed
        .get("context_text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        body.contains("Wire worker deployment config"),
        "opencode context missing checkpoint task: {body}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn precompact_checkpoint_round_trips_through_resume() {
    let home = fresh_home("resume");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    let checkpoint = native_with_cwd("claude-precompact.json", &project_dir);
    let ingested = run_cli(
        &home,
        &[
            "hook",
            "ingest",
            "--host",
            "claude-code",
            "--event",
            "PreCompact",
        ],
        Some(&checkpoint),
    );
    assert!(ingested.status.success());
    let resumed = run_cli(
        &home,
        &[
            "session",
            "resume",
            "--project",
            &project_dir.to_string_lossy(),
            "--json",
        ],
        None,
    );
    assert!(resumed.status.success());
    let value: serde_json::Value = serde_json::from_slice(&resumed.stdout).expect("resume json");
    let state_text = value
        .pointer("/working_state/state")
        .map(|v| v.to_string())
        .unwrap_or_default();
    assert!(state_text.contains("Wire worker deployment config"));
    assert!(state_text.contains("Waiting on staging credentials"));
    let summary = value
        .pointer("/last_episode/summary")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(summary.contains("Apply migration"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn hook_context_fails_open_under_lock_within_budget() {
    let home = fresh_home("hooktimeout");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    let db_path = home.join("memory.db");
    let holder = rusqlite::Connection::open(&db_path).expect("open holder");
    holder
        .execute_batch("BEGIN EXCLUSIVE;")
        .expect("take exclusive lock");
    let start = native_with_cwd("codex-sessionstart.json", &project_dir);
    let began = Instant::now();
    let context = run_cli(
        &home,
        &[
            "hook",
            "context",
            "--host",
            "codex",
            "--event",
            "SessionStart",
            "--timeout-ms",
            "1500",
        ],
        Some(&start),
    );
    let elapsed = began.elapsed();
    drop(holder);
    assert!(
        context.status.success(),
        "hook must fail open: {}",
        String::from_utf8_lossy(&context.stderr)
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "hook exceeded budget: {elapsed:?}"
    );
    assert!(
        context.stdout.is_empty()
            || String::from_utf8_lossy(&context.stdout).contains("No stored context"),
        "fail-open output must be empty or neutral"
    );
    let ingest = run_cli(
        &home,
        &["hook", "ingest", "--host", "codex", "--event", "Stop"],
        Some(&native_with_cwd(
            "codex-posttooluse-transcript.json",
            &project_dir,
        )),
    );
    assert!(ingest.status.success());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn secrets_are_scrubbed_before_storage() {
    let home = fresh_home("secrets");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    let failing = serde_json::json!({
        "session_id": "s-secure-1",
        "cwd": project_dir.to_string_lossy(),
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_use_id": "call-secure-1",
        "tool_input": {"command": "cargo test"},
        "tool_response": {"exit_code": 1, "output": "FAILED with token ghp_fixture0000000000000000000000"},
    });
    let ingested = run_cli(
        &home,
        &[
            "hook",
            "ingest",
            "--host",
            "codex",
            "--event",
            "PostToolUse",
        ],
        Some(&serde_json::to_vec(&failing).expect("json")),
    );
    assert!(ingested.status.success());
    let raw: Vec<u8> = std::fs::read(home.join("memory.db")).expect("read db");
    assert!(!raw
        .windows(36)
        .any(|w| w == b"ghp_fixture0000000000000000000000"));
    let conn = rusqlite::Connection::open(home.join("memory.db")).expect("open db");
    let payload: String = conn
        .query_row(
            "SELECT sanitized_payload FROM session_events LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("payload row");
    assert!(payload.contains("REDACTED"));
    assert!(!payload.contains("ghp_fixture0000000000000000000000"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn hermes_session_visible_from_claude() {
    let home = fresh_home("hermeshandoff");
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    let posted = native_with_cwd("hermes-post-llm.json", &project_dir);
    let ingested = run_cli(
        &home,
        &[
            "hook",
            "ingest",
            "--host",
            "hermes",
            "--event",
            "post_llm_call",
        ],
        Some(&posted),
    );
    assert!(
        ingested.status.success(),
        "hermes ingest failed: {}",
        String::from_utf8_lossy(&ingested.stderr)
    );
    let recall = native_with_cwd("hermes-pre-llm.json", &project_dir);
    let context = run_cli(
        &home,
        &[
            "hook",
            "context",
            "--host",
            "hermes",
            "--event",
            "pre_llm_call",
        ],
        Some(&recall),
    );
    assert!(context.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&context.stdout))
        .expect("hermes context json");
    let body = parsed.get("context").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        body.contains("staging credentials"),
        "hermes context missing assistant excerpt: {body}"
    );
    assert!(
        body.contains("untrusted"),
        "hermes context missing injection guard: {body}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn installer_preserves_host_config_through_cli() {
    let lam_home = fresh_home("installcli");
    let host_home = fresh_home("installhost");
    std::fs::create_dir_all(host_home.join(".codex")).expect("codex dir");
    std::fs::write(
        host_home.join(".codex").join("hooks.json"),
        serde_json::json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{"type": "command", "command": "echo keep-me"}],
                }],
            },
        })
        .to_string(),
    )
    .expect("seed hooks");
    let install = run_cli(
        &lam_home,
        &[
            "integration",
            "install",
            "codex",
            "--host-home",
            &host_home.to_string_lossy(),
        ],
        None,
    );
    assert!(
        install.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );
    let hooks_text =
        std::fs::read_to_string(host_home.join(".codex").join("hooks.json")).expect("hooks");
    assert!(hooks_text.contains("echo keep-me"));
    assert!(hooks_text.contains("agent-memory"));
    let status = run_cli(
        &lam_home,
        &[
            "integration",
            "status",
            "codex",
            "--host-home",
            &host_home.to_string_lossy(),
            "--json",
        ],
        None,
    );
    assert!(status.status.success());
    let uninstall = run_cli(
        &lam_home,
        &[
            "integration",
            "uninstall",
            "codex",
            "--host-home",
            &host_home.to_string_lossy(),
        ],
        None,
    );
    assert!(uninstall.status.success());
    let after =
        std::fs::read_to_string(host_home.join(".codex").join("hooks.json")).expect("hooks");
    assert!(after.contains("echo keep-me"));
    assert!(!after.contains("agent-memory"));
    let db_path = lam_home.join("memory.db");
    assert!(!db_path.exists() || rusqlite::Connection::open(&db_path).is_ok());
    let _ = std::fs::remove_dir_all(&lam_home);
    let _ = std::fs::remove_dir_all(&host_home);
}

#[test]
fn auto_install_detects_present_hosts_only() {
    let lam_home = fresh_home("autoinstall");
    let host_home = fresh_home("autohost");
    let empty_bin = fresh_home("autobin");
    std::fs::create_dir_all(host_home.join(".codex")).expect("codex dir");
    std::fs::create_dir_all(host_home.join(".claude")).expect("claude dir");
    let output = Command::new(binary())
        .args([
            "--home",
            &lam_home.to_string_lossy(),
            "--json",
            "integration",
            "install",
            "--host-home",
            &host_home.to_string_lossy(),
        ])
        .env("AGENT_MEMORY_HOME", &lam_home)
        .env("PATH", &empty_bin)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HERMES_HOME")
        .output()
        .expect("run auto install");
    assert!(
        output.status.success(),
        "auto install failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("auto install json");
    let hosts: Vec<&str> = rows
        .as_array()
        .expect("auto install returns an array")
        .iter()
        .filter_map(|row| row.get("host").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(hosts, vec!["codex", "claude-code"]);
    assert!(host_home.join(".codex").join("hooks.json").exists());
    assert!(host_home.join(".codex").join("config.toml").exists());
    assert!(host_home.join(".claude").join("settings.json").exists());
    assert!(host_home.join(".claude.json").exists());
    assert!(!host_home.join(".config").join("opencode").exists());
    assert!(!host_home.join(".hermes").exists());
    let _ = std::fs::remove_dir_all(&lam_home);
    let _ = std::fs::remove_dir_all(&host_home);
    let _ = std::fs::remove_dir_all(&empty_bin);
}

#[test]
fn auto_install_reports_when_no_hosts_detected() {
    let lam_home = fresh_home("autoempty");
    let host_home = fresh_home("autoemptyhost");
    let empty_bin = fresh_home("autoemptybin");
    let output = Command::new(binary())
        .args([
            "--home",
            &lam_home.to_string_lossy(),
            "--json",
            "integration",
            "install",
            "--host-home",
            &host_home.to_string_lossy(),
        ])
        .env("AGENT_MEMORY_HOME", &lam_home)
        .env("PATH", &empty_bin)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HERMES_HOME")
        .output()
        .expect("run auto install");
    assert!(output.status.success());
    let rows: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("auto install json");
    assert_eq!(rows.as_array().map(|items| items.len()), Some(0));
    assert!(!host_home.join(".codex").exists());
    assert!(!host_home.join(".claude").exists());
    let _ = std::fs::remove_dir_all(&lam_home);
    let _ = std::fs::remove_dir_all(&host_home);
    let _ = std::fs::remove_dir_all(&empty_bin);
}
