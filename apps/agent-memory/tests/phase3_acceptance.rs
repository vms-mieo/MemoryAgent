use std::io::BufRead;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agent-memory"))
}

fn fresh_home(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "lam-phase3-{}-{}-{}",
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

fn run_cli(home: &Path, args: &[&str]) -> std::process::Output {
    let mut full: Vec<String> = vec!["--home".to_string(), home.to_string_lossy().to_string()];
    for item in args {
        full.push(item.to_string());
    }
    Command::new(binary())
        .args(full)
        .env("AGENT_MEMORY_HOME", home)
        .output()
        .expect("spawn agent-memory")
}

use std::process::Command;

fn json_output(output: &std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout json")
}

fn setup_project(home: &Path, name: &str) -> PathBuf {
    assert!(run_cli(home, &["init"]).status.success());
    let dir = home.join(name);
    std::fs::create_dir_all(&dir).expect("project dir");
    dir
}

#[test]
fn historical_as_of_returns_before_and_after_migration() {
    let home = fresh_home("temporal");
    let project_dir = setup_project(&home, "repo");
    let project = project_dir.to_string_lossy().to_string();
    let mysql = run_cli(
        &home,
        &[
            "memory",
            "assert",
            "--project",
            &project,
            "--subject",
            "database.engine",
            "--predicate",
            "uses",
            "--value",
            "MySQL",
            "--source-ref",
            "docs/adr/0001-mysql.md",
            "--valid-from",
            "2026-10-20T09:00:00Z",
        ],
    );
    assert!(
        mysql.status.success(),
        "mysql assert failed: {}",
        String::from_utf8_lossy(&mysql.stderr)
    );
    let pg = run_cli(
        &home,
        &[
            "memory",
            "assert",
            "--project",
            &project,
            "--subject",
            "database.engine",
            "--predicate",
            "uses",
            "--value",
            "PostgreSQL",
            "--source-ref",
            "docs/adr/0003-postgres.md",
            "--valid-from",
            "2026-10-22T09:00:00Z",
        ],
    );
    assert!(
        pg.status.success(),
        "pg assert failed: {}",
        String::from_utf8_lossy(&pg.stderr)
    );
    let before = json_output(&run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &project,
            "--subject",
            "database.engine",
            "--predicate",
            "uses",
            "--as-of",
            "2026-10-21T09:00:00Z",
            "--json",
        ],
    ));
    assert_eq!(
        before.pointer("/active/value").and_then(|v| v.as_str()),
        Some("MySQL")
    );
    let after = json_output(&run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &project,
            "--subject",
            "database.engine",
            "--predicate",
            "uses",
            "--as-of",
            "2026-10-23T09:00:00Z",
            "--json",
        ],
    ));
    assert_eq!(
        after.pointer("/active/value").and_then(|v| v.as_str()),
        Some("PostgreSQL")
    );
    let timeline = json_output(&run_cli(
        &home,
        &[
            "memory",
            "timeline",
            "--project",
            &project,
            "--subject",
            "database.engine",
            "--predicate",
            "uses",
            "--json",
        ],
    ));
    let items = timeline.as_array().expect("timeline array");
    assert_eq!(items.len(), 2);
    let mysql_item = items
        .iter()
        .find(|item| item.pointer("/value").and_then(|v| v.as_str()) == Some("MySQL"))
        .expect("mysql revision");
    assert_eq!(
        mysql_item.pointer("/valid_until").and_then(|v| v.as_str()),
        Some("2026-10-22T09:00:00+00:00")
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn low_confidence_candidate_never_silently_overwrites() {
    let home = fresh_home("conflict");
    let project_dir = setup_project(&home, "repo");
    let project = project_dir.to_string_lossy().to_string();
    let verified = run_cli(
        &home,
        &[
            "memory",
            "assert",
            "--project",
            &project,
            "--subject",
            "provisioning.orchestrator",
            "--predicate",
            "uses",
            "--value",
            "Temporal",
            "--source-ref",
            "docs/adr/0003-use-temporal.md",
            "--valid-from",
            "2026-10-20T09:00:00Z",
        ],
    );
    assert!(verified.status.success());
    let candidate = run_cli(
        &home,
        &[
            "memory",
            "assert",
            "--project",
            &project,
            "--subject",
            "provisioning.orchestrator",
            "--predicate",
            "uses",
            "--value",
            "custom-cron",
            "--status",
            "proposed",
            "--actor",
            "extract:test",
            "--confidence",
            "0.4",
            "--valid-from",
            "2026-10-21T09:00:00Z",
        ],
    );
    assert!(
        candidate.status.success(),
        "candidate failed: {}",
        String::from_utf8_lossy(&candidate.stderr)
    );
    let conflicts = json_output(&run_cli(
        &home,
        &["memory", "conflicts", "--project", &project, "--json"],
    ));
    assert_eq!(conflicts.as_array().map(|a| a.len()), Some(1));
    let active = json_output(&run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &project,
            "--subject",
            "provisioning.orchestrator",
            "--predicate",
            "uses",
            "--json",
        ],
    ));
    assert_eq!(
        active.pointer("/active/value").and_then(|v| v.as_str()),
        Some("Temporal")
    );
    let search = json_output(&run_cli(
        &home,
        &[
            "memory",
            "search",
            "--project",
            &project,
            "--query",
            "custom-cron",
            "--json",
        ],
    ));
    assert_eq!(search.as_array().map(|a| a.len()), Some(0));
    let conflict_id = conflicts
        .get(0)
        .and_then(|item| item.get("conflict_id"))
        .and_then(|v| v.as_str())
        .expect("conflict id")
        .to_string();
    let resolved = run_cli(
        &home,
        &[
            "memory",
            "resolve",
            &conflict_id,
            "--keep",
            "active",
            "--reason",
            "adr wins",
        ],
    );
    assert!(
        resolved.status.success(),
        "resolve failed: {}",
        String::from_utf8_lossy(&resolved.stderr)
    );
    let after = json_output(&run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &project,
            "--subject",
            "provisioning.orchestrator",
            "--predicate",
            "uses",
            "--json",
        ],
    ));
    assert_eq!(
        after.pointer("/active/value").and_then(|v| v.as_str()),
        Some("Temporal")
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn graph_traversal_respects_validity_window() {
    let home = fresh_home("graph");
    let project_dir = setup_project(&home, "repo");
    let project = project_dir.to_string_lossy().to_string();
    let kafka = json_output(&run_cli(
        &home,
        &[
            "memory",
            "graph",
            "entity-add",
            "--project",
            &project,
            "--name",
            "Kafka",
            "--kind",
            "service",
            "--json",
        ],
    ));
    let api = json_output(&run_cli(
        &home,
        &[
            "memory",
            "graph",
            "entity-add",
            "--project",
            &project,
            "--name",
            "api",
            "--kind",
            "service",
            "--json",
        ],
    ));
    let kafka_id = kafka
        .get("entity_id")
        .and_then(|v| v.as_str())
        .expect("kafka id")
        .to_string();
    let api_id = api
        .get("entity_id")
        .and_then(|v| v.as_str())
        .expect("api id")
        .to_string();
    let edge = run_cli(
        &home,
        &[
            "memory",
            "graph",
            "edge-add",
            "--project",
            &project,
            "--source",
            &api_id,
            "--relation",
            "depends_on",
            "--target",
            &kafka_id,
            "--valid-from",
            "2026-10-01T09:00:00Z",
            "--valid-until",
            "2026-10-20T09:00:00Z",
        ],
    );
    assert!(
        edge.status.success(),
        "edge failed: {}",
        String::from_utf8_lossy(&edge.stderr)
    );
    let during = json_output(&run_cli(
        &home,
        &[
            "memory",
            "graph",
            "traverse",
            "--project",
            &project,
            "--entity",
            &api_id,
            "--depth",
            "1",
            "--as-of",
            "2026-10-10T09:00:00Z",
            "--json",
        ],
    ));
    assert_eq!(
        during
            .pointer("/edges")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(1)
    );
    let after = json_output(&run_cli(
        &home,
        &[
            "memory",
            "graph",
            "traverse",
            "--project",
            &project,
            "--entity",
            &api_id,
            "--depth",
            "1",
            "--as-of",
            "2026-10-21T09:00:00Z",
            "--json",
        ],
    ));
    assert_eq!(
        after
            .pointer("/edges")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(0)
    );
    assert_eq!(
        after
            .pointer("/nodes")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(1)
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn fact_scope_isolation_across_projects() {
    let home = fresh_home("scope");
    assert!(run_cli(&home, &["init"]).status.success());
    let dir_a = home.join("proj-a");
    let dir_b = home.join("proj-b");
    std::fs::create_dir_all(&dir_a).expect("a");
    std::fs::create_dir_all(&dir_b).expect("b");
    let a = dir_a.to_string_lossy().to_string();
    let b = dir_b.to_string_lossy().to_string();
    assert!(run_cli(
        &home,
        &[
            "memory",
            "assert",
            "--project",
            &a,
            "--subject",
            "cache.backend",
            "--predicate",
            "uses",
            "--value",
            "redis"
        ]
    )
    .status
    .success());
    assert!(run_cli(
        &home,
        &[
            "memory",
            "assert",
            "--project",
            &b,
            "--subject",
            "cache.backend",
            "--predicate",
            "uses",
            "--value",
            "memcached"
        ]
    )
    .status
    .success());
    let timeline_a = json_output(&run_cli(
        &home,
        &[
            "memory",
            "timeline",
            "--project",
            &a,
            "--subject",
            "cache.backend",
            "--predicate",
            "uses",
            "--json",
        ],
    ));
    assert_eq!(timeline_a.as_array().map(|items| items.len()), Some(1));
    assert_eq!(
        timeline_a
            .get(0)
            .and_then(|item| item.get("value"))
            .and_then(|v| v.as_str()),
        Some("redis")
    );
    let fact_b = json_output(&run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &b,
            "--subject",
            "cache.backend",
            "--predicate",
            "uses",
            "--json",
        ],
    ));
    assert_eq!(
        fact_b.pointer("/active/value").and_then(|v| v.as_str()),
        Some("memcached")
    );
    let search_a = json_output(&run_cli(
        &home,
        &[
            "memory",
            "search",
            "--project",
            &a,
            "--query",
            "memcached",
            "--json",
        ],
    ));
    assert_eq!(search_a.as_array().map(|items| items.len()), Some(0));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn environment_slots_stay_independent() {
    let home = fresh_home("env");
    let project_dir = setup_project(&home, "repo");
    let project = project_dir.to_string_lossy().to_string();
    assert!(run_cli(
        &home,
        &[
            "memory",
            "assert",
            "--project",
            &project,
            "--subject",
            "workers",
            "--predicate",
            "replicas",
            "--value",
            "3",
            "--value-json",
            "--env",
            "prod"
        ]
    )
    .status
    .success());
    assert!(run_cli(
        &home,
        &[
            "memory",
            "assert",
            "--project",
            &project,
            "--subject",
            "workers",
            "--predicate",
            "replicas",
            "--value",
            "1",
            "--value-json",
            "--env",
            "staging"
        ]
    )
    .status
    .success());
    let prod = json_output(&run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &project,
            "--subject",
            "workers",
            "--predicate",
            "replicas",
            "--env",
            "prod",
            "--json",
        ],
    ));
    assert_eq!(prod.pointer("/active/value"), Some(&serde_json::json!(3)));
    let staging = json_output(&run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &project,
            "--subject",
            "workers",
            "--predicate",
            "replicas",
            "--env",
            "staging",
            "--json",
        ],
    ));
    assert_eq!(
        staging.pointer("/active/value"),
        Some(&serde_json::json!(1))
    );
    let _ = std::fs::remove_dir_all(&home);
}

struct McpSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    responses: mpsc::Receiver<String>,
}

fn start_mcp(home: &Path) -> McpSession {
    let mut child = Command::new(binary())
        .args(["--home", &home.to_string_lossy(), "mcp"])
        .env("AGENT_MEMORY_HOME", home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn mcp server");
    let stdin = child.stdin.take().expect("mcp stdin");
    let stdout = child.stdout.take().expect("mcp stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(text) => {
                    if tx.send(text).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    McpSession {
        child,
        stdin,
        responses: rx,
    }
}

fn mcp_call(
    session: &mut McpSession,
    id: u64,
    tool: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    use std::io::Write;
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    });
    let mut text = serde_json::to_string(&message).expect("serialize request");
    text.push('\n');
    session
        .stdin
        .write_all(text.as_bytes())
        .expect("write mcp request");
    session.stdin.flush().expect("flush mcp request");
    loop {
        let line = session
            .responses
            .recv_timeout(Duration::from_secs(20))
            .expect("mcp response within timeout");
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).expect("response json");
        if value.get("id") == Some(&serde_json::json!(id)) {
            assert!(value.get("error").is_none(), "tool {tool} failed: {value}");
            let inner = value
                .pointer("/result/content/0/text")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("tool {tool} returned no text content: {value}"));
            return serde_json::from_str(inner)
                .unwrap_or_else(|_| panic!("tool {tool} text is not json: {inner}"));
        }
    }
}

fn mcp_handshake(session: &mut McpSession) {
    use std::io::Write;
    for (id, method, params) in [
        (
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "phase3-test", "version": "0.1.0"},
            }),
        ),
        (2, "tools/list", serde_json::json!({})),
    ] {
        let message =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut text = serde_json::to_string(&message).expect("serialize");
        text.push('\n');
        session.stdin.write_all(text.as_bytes()).expect("write");
    }
    session
        .stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .expect("notify");
    session.stdin.flush().expect("flush");
    let mut seen = 0;
    while seen < 2 {
        let line = session
            .responses
            .recv_timeout(Duration::from_secs(20))
            .expect("handshake");
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).expect("handshake json");
        if value.get("id").is_some() {
            seen += 1;
        }
    }
}

#[test]
fn mcp_timeline_and_assert_round_trip() {
    let home = fresh_home("mcptemporal");
    let project_dir = setup_project(&home, "repo");
    let project = project_dir.to_string_lossy().to_string();
    let mut session = start_mcp(&home);
    mcp_handshake(&mut session);
    let saved = mcp_call(
        &mut session,
        3,
        "memory_assert",
        serde_json::json!({
            "subject": "database.engine",
            "predicate": "uses",
            "value": "MySQL",
            "project_path": project,
            "valid_from": "2026-10-20T09:00:00Z",
            "source_ref": "docs/adr/0001.md",
        }),
    );
    assert!(saved.get("assertion_id").and_then(|v| v.as_str()).is_some());
    assert!(saved.get("conflict_id").and_then(|v| v.as_str()).is_none());
    let timeline = mcp_call(
        &mut session,
        4,
        "memory_timeline",
        serde_json::json!({
            "subject": "database.engine",
            "predicate": "uses",
            "project_path": project,
        }),
    );
    assert_eq!(
        timeline
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(1)
    );
    let weaker = mcp_call(
        &mut session,
        5,
        "memory_assert",
        serde_json::json!({
            "subject": "database.engine",
            "predicate": "uses",
            "value": "PostgreSQL",
            "project_path": project,
            "status": "proposed",
            "actor": "extract:test",
            "confidence": 0.4,
            "valid_from": "2026-10-22T09:00:00Z",
        }),
    );
    assert!(weaker.get("conflict_id").and_then(|v| v.as_str()).is_some());
    let _ = session.child.kill();
    let _ = session.child.wait();
    let _ = std::fs::remove_dir_all(&home);
}
