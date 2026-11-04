use std::io::BufRead;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::ChildStdin;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agent-memory"))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn fresh_home(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "lam-phase1-{}-{}-{}",
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

struct McpSession {
    child: Child,
    stdin: ChildStdin,
    responses: mpsc::Receiver<String>,
    lines: Vec<String>,
}

fn start_mcp(home: &Path) -> McpSession {
    let mut child = Command::new(binary())
        .args(["--home", &home.to_string_lossy(), "mcp"])
        .env("AGENT_MEMORY_HOME", home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
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
        lines: Vec::new(),
    }
}

fn mcp_request(
    session: &mut McpSession,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
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
        session.lines.push(line.clone());
        let value: serde_json::Value = serde_json::from_str(&line).expect("response is json");
        if value.get("id") == Some(&serde_json::json!(id)) {
            return value;
        }
    }
}

fn mcp_notify(session: &mut McpSession, method: &str) {
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
    });
    let mut text = serde_json::to_string(&message).expect("serialize notification");
    text.push('\n');
    session
        .stdin
        .write_all(text.as_bytes())
        .expect("write notification");
    session.stdin.flush().expect("flush notification");
}

fn mcp_call(
    session: &mut McpSession,
    id: u64,
    tool: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let response = mcp_request(
        session,
        id,
        "tools/call",
        serde_json::json!({"name": tool, "arguments": arguments}),
    );
    assert!(
        response.get("error").is_none(),
        "tool {tool} failed: {response}"
    );
    let text = response
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("tool {tool} returned no text content: {response}"));
    serde_json::from_str(text).unwrap_or_else(|_| panic!("tool {tool} text is not json: {text}"))
}

fn stop_mcp(mut session: McpSession) {
    let _ = session.child.kill();
    let _ = session.child.wait();
    for line in &session.lines {
        let value: serde_json::Value =
            serde_json::from_str(line).expect("every stdout line is MCP JSON-RPC");
        assert_eq!(value.get("jsonrpc").and_then(|v| v.as_str()), Some("2.0"));
    }
}

fn initialize_session(session: &mut McpSession) {
    let response = mcp_request(
        session,
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "phase1-test", "version": "0.1.0"},
        }),
    );
    assert!(
        response.get("result").is_some(),
        "initialize failed: {response}"
    );
    mcp_notify(session, "notifications/initialized");
    let tools = mcp_request(session, 2, "tools/list", serde_json::json!({}));
    let names: Vec<String> = tools
        .pointer("/result/tools")
        .and_then(|v| v.as_array())
        .expect("tools list")
        .iter()
        .filter_map(|t| {
            t.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    for wanted in [
        "memory_search",
        "memory_get",
        "memory_save",
        "memory_context",
        "memory_review",
        "memory_forget",
    ] {
        assert!(names.contains(&wanted.to_string()), "missing tool {wanted}");
    }
}

#[test]
fn mcp_client_reads_and_writes_real_rows() {
    let home = fresh_home("mcp-real");
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    let mut session = start_mcp(&home);
    initialize_session(&mut session);
    let saved = mcp_call(
        &mut session,
        3,
        "memory_save",
        serde_json::json!({
            "content": "Phase1 MCP round trip over stdio",
            "kind": "fact",
            "project_path": project_dir.to_string_lossy(),
            "scope": "project",
            "sources": [{"kind": "document", "path": "docs/phase1.md", "line_start": 1, "line_end": 5}],
            "idempotency_key": "phase1-key-1",
            "actor": "phase1-test",
        }),
    );
    let memory_id = saved
        .get("memory_id")
        .and_then(|v| v.as_str())
        .expect("save returns memory_id")
        .to_string();
    let replayed = mcp_call(
        &mut session,
        4,
        "memory_save",
        serde_json::json!({
            "content": "Different text but same key",
            "kind": "fact",
            "project_path": project_dir.to_string_lossy(),
            "idempotency_key": "phase1-key-1",
            "actor": "phase1-test",
        }),
    );
    assert_eq!(
        replayed.get("memory_id").and_then(|v| v.as_str()),
        Some(memory_id.as_str())
    );
    let found = mcp_call(
        &mut session,
        5,
        "memory_search",
        serde_json::json!({
            "query": "Phase1 round trip",
            "project_path": project_dir.to_string_lossy(),
            "limit": 5,
        }),
    );
    let items = found
        .get("items")
        .and_then(|v| v.as_array())
        .expect("search items");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].get("memory_id").and_then(|v| v.as_str()),
        Some(memory_id.as_str())
    );
    let detail = mcp_call(
        &mut session,
        6,
        "memory_get",
        serde_json::json!({"memory_id": memory_id}),
    );
    assert_eq!(
        detail.get("content").and_then(|v| v.as_str()),
        Some("Phase1 MCP round trip over stdio")
    );
    assert_eq!(
        detail
            .pointer("/sources/0/source_ref")
            .and_then(|v| v.as_str()),
        Some("docs/phase1.md")
    );
    let context = mcp_call(
        &mut session,
        7,
        "memory_context",
        serde_json::json!({
            "task": "verify MCP round trip works",
            "project_path": project_dir.to_string_lossy(),
            "token_budget": 200,
        }),
    );
    assert_eq!(
        context.get("schema_version").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert!(!context
        .get("items")
        .and_then(|v| v.as_array())
        .expect("context items")
        .is_empty());
    stop_mcp(session);
    let conn = rusqlite::Connection::open(home.join("memory.db")).expect("open db written by MCP");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
        .expect("count memories");
    assert_eq!(count, 1);
    lam_storage::integrity_check(&conn).expect("integrity ok");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn fts_query_returns_path_lines_and_provenance() {
    let home = fresh_home("fts");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"]).status.success());
    let add = run_cli(
        &home,
        &[
            "memory",
            "add",
            "--project",
            &project_dir.to_string_lossy(),
            "--kind",
            "fact",
            "--text",
            "Temporal provisions the database workflow engine",
            "--source-ref",
            "docs/adr/0003-use-temporal.md",
            "--line-start",
            "12",
            "--line-end",
            "31",
        ],
    );
    assert!(
        add.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let search = run_cli(
        &home,
        &[
            "memory",
            "search",
            "--project",
            &project_dir.to_string_lossy(),
            "--query",
            "Temporal provisioning",
            "--json",
        ],
    );
    assert!(search.status.success());
    let hits: serde_json::Value = serde_json::from_slice(&search.stdout).expect("search json");
    let first = hits.get(0).expect("one hit");
    assert_eq!(
        first.get("source").and_then(|v| v.as_str()),
        Some("docs/adr/0003-use-temporal.md:12-31")
    );
    assert!(first
        .get("snippet")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains("Temporal"));
    let show_id = first
        .get("memory_id")
        .and_then(|v| v.as_str())
        .expect("memory id")
        .to_string();
    let proposed = run_cli(
        &home,
        &[
            "memory",
            "add",
            "--project",
            &project_dir.to_string_lossy(),
            "--kind",
            "lesson",
            "--status",
            "proposed",
            "--text",
            "Hypothesis about provisioning retries",
        ],
    );
    assert!(proposed.status.success());
    let review = run_cli(
        &home,
        &[
            "memory",
            "review",
            "--list",
            "--project",
            &project_dir.to_string_lossy(),
            "--json",
        ],
    );
    assert!(review.status.success());
    let queue: serde_json::Value = serde_json::from_slice(&review.stdout).expect("review json");
    assert_eq!(queue.as_array().map(|a| a.len()), Some(1));
    let approve = run_cli(
        &home,
        &["memory", "approve", &show_id, "--reason", "adr confirmed"],
    );
    assert!(approve.status.success());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn private_records_never_leak_across_projects() {
    let home = fresh_home("isolation");
    let dir_a = home.join("proj-a");
    let dir_b = home.join("proj-b");
    std::fs::create_dir_all(&dir_a).expect("dir a");
    std::fs::create_dir_all(&dir_b).expect("dir b");
    assert!(run_cli(&home, &["init"]).status.success());
    let bind_a = run_cli(
        &home,
        &[
            "project",
            "bind",
            "--path",
            &dir_a.to_string_lossy(),
            "--name",
            "proj-a",
        ],
    );
    assert!(bind_a.status.success());
    let save_b_private = run_cli(
        &home,
        &[
            "memory",
            "add",
            "--project",
            &dir_b.to_string_lossy(),
            "--kind",
            "fact",
            "--scope",
            "agent-private",
            "--text",
            "B private blueprint for vault rotation",
        ],
    );
    assert!(save_b_private.status.success());
    let save_b_project = run_cli(
        &home,
        &[
            "memory",
            "add",
            "--project",
            &dir_b.to_string_lossy(),
            "--kind",
            "fact",
            "--text",
            "B project blueprint for vault rotation",
        ],
    );
    assert!(save_b_project.status.success());
    let save_global = run_cli(
        &home,
        &[
            "memory",
            "add",
            "--project",
            &dir_b.to_string_lossy(),
            "--kind",
            "convention",
            "--scope",
            "global",
            "--text",
            "Shared global blueprint for vault rotation",
        ],
    );
    assert!(save_global.status.success());
    let search_a = run_cli(
        &home,
        &[
            "memory",
            "search",
            "--project",
            &dir_a.to_string_lossy(),
            "--query",
            "blueprint vault rotation",
            "--json",
        ],
    );
    assert!(search_a.status.success());
    let hits_a: serde_json::Value =
        serde_json::from_slice(&search_a.stdout).expect("search a json");
    let texts: Vec<String> = hits_a
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|h| {
            h.get("snippet")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(
        texts
            .iter()
            .all(|t| !t.contains("B private") && !t.contains("B project")),
        "cross-project leak: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("Shared global")),
        "global memory should be visible: {texts:?}"
    );
    let search_b_default = run_cli(
        &home,
        &[
            "memory",
            "search",
            "--project",
            &dir_b.to_string_lossy(),
            "--query",
            "blueprint vault rotation",
            "--json",
        ],
    );
    assert!(search_b_default.status.success());
    let hits_b: serde_json::Value =
        serde_json::from_slice(&search_b_default.stdout).expect("search b json");
    let texts_b: Vec<String> = hits_b
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|h| {
            h.get("snippet")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(
        texts_b.iter().all(|t| !t.contains("B private")),
        "private must stay hidden by default: {texts_b:?}"
    );
    let search_b_private = run_cli(
        &home,
        &[
            "memory",
            "search",
            "--project",
            &dir_b.to_string_lossy(),
            "--query",
            "blueprint vault rotation",
            "--scope",
            "agent-private",
            "--json",
        ],
    );
    assert!(search_b_private.status.success());
    let hits_p: serde_json::Value =
        serde_json::from_slice(&search_b_private.stdout).expect("private json");
    assert_eq!(hits_p.as_array().map(|a| a.len()), Some(1));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn secret_corpus_is_rejected_before_storage() {
    let corpus_path = workspace_root()
        .join("fixtures")
        .join("secrets")
        .join("secret-corpus.json");
    let text = std::fs::read_to_string(&corpus_path).expect("read secret corpus");
    let corpus: serde_json::Value = serde_json::from_str(&text).expect("corpus json");
    let cases = corpus
        .get("cases")
        .and_then(|v| v.as_array())
        .expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        let raw = case.get("raw").and_then(|v| v.as_str()).expect("raw");
        let redacted = case
            .get("redacted")
            .and_then(|v| v.as_str())
            .expect("redacted");
        assert!(
            lam_core::ensure_no_secrets(raw).is_err(),
            "raw fixture must be rejected: {raw}"
        );
        assert!(
            lam_core::ensure_no_secrets(redacted).is_ok(),
            "redacted fixture must pass: {redacted}"
        );
    }
    let home = fresh_home("secrets");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"]).status.success());
    let denied = run_cli(
        &home,
        &[
            "memory",
            "add",
            "--project",
            &project_dir.to_string_lossy(),
            "--kind",
            "fact",
            "--text",
            "token ghp_fixture0000000000000000000000000000",
        ],
    );
    assert!(!denied.status.success(), "secret save must fail");
    let conn = rusqlite::Connection::open(home.join("memory.db")).expect("open db after denial");
    let raw: Vec<u8> = std::fs::read(home.join("memory.db")).expect("read db bytes");
    assert!(
        !raw.windows(9).any(|w| w == b"ghp_fixt"),
        "secret bytes must not reach storage"
    );
    lam_storage::integrity_check(&conn).expect("integrity ok");
    let _ = std::fs::remove_dir_all(&home);
}

#[cfg(unix)]
#[test]
fn kill_during_write_keeps_integrity() {
    let home = fresh_home("killwrite");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"]).status.success());
    let baseline = run_cli(
        &home,
        &[
            "memory",
            "add",
            "--project",
            &project_dir.to_string_lossy(),
            "--kind",
            "fact",
            "--text",
            "Baseline memory before kill storm",
        ],
    );
    assert!(baseline.status.success());
    let mut children: Vec<Child> = Vec::new();
    for index in 0..6 {
        let child = Command::new(binary())
            .args([
                "--home",
                &home.to_string_lossy(),
                "memory",
                "add",
                "--project",
                &project_dir.to_string_lossy(),
                "--kind",
                "fact",
                "--text",
                &format!("Concurrent write candidate {index}"),
            ])
            .env("AGENT_MEMORY_HOME", &home)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn writer");
        children.push(child);
    }
    std::thread::sleep(Duration::from_millis(50));
    let mut killed = 0;
    for mut child in children.drain(..3) {
        let pid = child.id().to_string();
        let _ = Command::new("kill").args(["-9", &pid]).output();
        let _ = child.wait();
        killed += 1;
    }
    assert_eq!(killed, 3);
    for mut child in children {
        let _ = child.wait();
    }
    let conn =
        rusqlite::Connection::open(home.join("memory.db")).expect("open db after kill storm");
    lam_storage::integrity_check(&conn).expect("integrity ok after kill");
    let doctor = run_cli(&home, &["doctor", "--json"]);
    assert!(
        doctor.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let search = run_cli(
        &home,
        &[
            "memory",
            "search",
            "--project",
            &project_dir.to_string_lossy(),
            "--query",
            "Baseline memory",
            "--json",
        ],
    );
    assert!(search.status.success());
    let hits: serde_json::Value = serde_json::from_slice(&search.stdout).expect("search json");
    assert_eq!(hits.as_array().map(|a| a.len()), Some(1));
    let _ = std::fs::remove_dir_all(&home);
}
