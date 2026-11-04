use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agent-memory"))
}

fn fresh_home(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "lam-phase5-{}-{}-{}",
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

fn checkpoint_event(session: &str, cwd: &Path, goal: &str, task: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "session_id": session,
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd.to_string_lossy(),
        "hook_event_name": "PreCompact",
        "goal": goal,
        "current_task": task,
        "completed_steps": ["reproduced on staging"],
        "next_steps": ["verify fix on staging"],
        "blockers": [],
        "files_touched": []
    }))
    .expect("event json")
}

fn ingest_checkpoint(home: &Path, event: &[u8]) -> std::process::Output {
    run_cli(
        home,
        &[
            "hook",
            "ingest",
            "--host",
            "claude-code",
            "--event",
            "PreCompact",
        ],
        Some(event),
    )
}

fn parse_stdout_json(output: &std::process::Output) -> serde_json::Value {
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("stdout is not json: {text}"))
}

fn set_config(home: &Path, replacements: &[(&str, &str)]) {
    let path = home.join("config.toml");
    let mut text = std::fs::read_to_string(&path).expect("read config");
    for (from, to) in replacements {
        assert!(text.contains(from), "config missing {from}");
        text = text.replacen(from, to, 1);
    }
    std::fs::write(&path, text).expect("write config");
}

fn review_items(home: &Path, project: &Path) -> Vec<serde_json::Value> {
    let output = run_cli(
        home,
        &[
            "memory",
            "review",
            "--list",
            "--project",
            &project.to_string_lossy(),
            "--json",
        ],
        None,
    );
    assert!(
        output.status.success(),
        "review failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = parse_stdout_json(&output);
    value.as_array().cloned().unwrap_or_default()
}

#[test]
fn ten_related_episodes_yield_one_proposed_lesson() {
    let home = fresh_home("ten-episodes");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    set_config(
        &home,
        &[
            ("min_new_episodes = 10", "min_new_episodes = 2"),
            ("min_idle_minutes = 10", "min_idle_minutes = 0"),
        ],
    );
    let goal = "Stabilize timeout handling in media path on staging";
    for (index, session) in [
        "dream-a", "dream-a", "dream-a", "dream-a", "dream-b", "dream-b", "dream-b", "dream-c",
        "dream-c", "dream-c",
    ]
    .iter()
    .enumerate()
    {
        let task = format!("Investigate timeout handling in media path batch {index}");
        let event = checkpoint_event(session, &project_dir, goal, &task);
        let ingested = ingest_checkpoint(&home, &event);
        assert!(
            ingested.status.success(),
            "ingest {index} failed: {}",
            String::from_utf8_lossy(&ingested.stderr)
        );
    }
    let maintained = run_cli(
        &home,
        &[
            "maintain",
            "--project",
            &project_dir.to_string_lossy(),
            "--budget-ms",
            "30000",
            "--json",
        ],
        None,
    );
    assert!(
        maintained.status.success(),
        "maintain failed: {}",
        String::from_utf8_lossy(&maintained.stderr)
    );
    let items = review_items(&home, &project_dir);
    let lessons: Vec<&serde_json::Value> = items
        .iter()
        .filter(|item| {
            item.get("kind").and_then(|v| v.as_str()) == Some("lesson")
                && item
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|text| text.contains("Recurring theme"))
                    .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        lessons.len(),
        1,
        "expected one dreamed lesson, got {}",
        items.len()
    );
    assert_eq!(
        lessons[0].get("status").and_then(|v| v.as_str()),
        Some("proposed"),
        "dreamed insight must stay proposed, never verified"
    );
    assert!(
        lessons[0]
            .get("content")
            .and_then(|v| v.as_str())
            .map(|text| text.contains("timeout") || text.contains("media path"))
            .unwrap_or(false),
        "dreamed lesson must link back to the shared episode theme"
    );
    assert_eq!(
        items.len(),
        1,
        "ten related episodes must consolidate into exactly one review item, got {}",
        items.len()
    );
}

#[test]
fn sigkill_crash_recovers_without_duplicate_insight() {
    let home = fresh_home("sigkill");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    let event = checkpoint_event(
        "kill-session",
        &project_dir,
        "Index the codebase with ripgrep",
        "Search uses ripgrep index for fast lookup",
    );
    let ingested = ingest_checkpoint(&home, &event);
    assert!(ingested.status.success());
    let mut child = Command::new(binary())
        .args([
            "--home",
            &home.to_string_lossy(),
            "maintain",
            "--project",
            &project_dir.to_string_lossy(),
            "--budget-ms",
            "30000",
        ])
        .env("AGENT_MEMORY_HOME", &home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn maintain");
    let crashed_mid_flight = match child.try_wait() {
        Ok(None) => {
            let killed = Command::new("kill")
                .args(["-9", &child.id().to_string()])
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            let exit = child.wait().expect("reap maintain child");
            assert!(killed, "could not deliver SIGKILL to maintain child");
            assert!(
                !exit.success(),
                "maintain child must die from the signal, not exit cleanly"
            );
            true
        }
        Ok(Some(status)) => {
            assert!(
                status.success(),
                "maintain child exited on its own with failure"
            );
            false
        }
        Err(e) => panic!("could not poll maintain child: {e}"),
    };
    let _ = crashed_mid_flight;
    let rerun = run_cli(
        &home,
        &[
            "maintain",
            "--project",
            &project_dir.to_string_lossy(),
            "--budget-ms",
            "30000",
            "--json",
        ],
        None,
    );
    assert!(
        rerun.status.success(),
        "rerun after SIGKILL failed: {}",
        String::from_utf8_lossy(&rerun.stderr)
    );
    let items = review_items(&home, &project_dir);
    let matches = items
        .iter()
        .filter(|item| {
            item.get("content")
                .and_then(|v| v.as_str())
                .map(|text| text.contains("ripgrep"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        matches, 2,
        "SIGKILL recovery must leave exactly the extracted note and its fact, no duplicates"
    );
    let fact_get = run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &project_dir.to_string_lossy(),
            "--subject",
            "Search",
            "--predicate",
            "uses",
            "--json",
        ],
        None,
    );
    assert!(fact_get.status.success());
    let value = parse_stdout_json(&fact_get);
    assert_eq!(
        value
            .get("active")
            .and_then(|entry| entry.get("value"))
            .and_then(|entry| entry.as_str()),
        Some("ripgrep"),
        "single surviving fact value expected: {}",
        String::from_utf8_lossy(&fact_get.stdout)
    );
}

#[test]
fn gate_falls_back_without_local_model() {
    let home = fresh_home("gate-fallback");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    set_config(
        &home,
        &[
            ("mode = \"shadow\"", "mode = \"gate\""),
            ("minimum_confidence = 0.85", "minimum_confidence = 0.6"),
        ],
    );
    let event = checkpoint_event(
        "gate-session",
        &project_dir,
        "Cache uses redis cluster for sessions",
        "Cache uses redis cluster for sessions",
    );
    let ingested = ingest_checkpoint(&home, &event);
    assert!(ingested.status.success());
    let maintained = run_cli(
        &home,
        &[
            "maintain",
            "--project",
            &project_dir.to_string_lossy(),
            "--budget-ms",
            "30000",
            "--json",
        ],
        None,
    );
    assert!(
        maintained.status.success(),
        "gate maintain without model must fall back, not fail: {}",
        String::from_utf8_lossy(&maintained.stderr)
    );
    let searched = run_cli(
        &home,
        &[
            "memory",
            "search",
            "--project",
            &project_dir.to_string_lossy(),
            "--query",
            "redis cluster sessions",
            "--include-history",
            "--json",
        ],
        None,
    );
    assert!(searched.status.success());
    let hits = parse_stdout_json(&searched);
    assert!(
        hits.as_array()
            .map(|list| !list.is_empty())
            .unwrap_or(false),
        "read gate must fall back to deterministic results instead of emptying them"
    );
    let items = review_items(&home, &project_dir);
    assert!(
        items.iter().any(|item| {
            item.get("content")
                .and_then(|v| v.as_str())
                .map(|text| text.contains("redis"))
                .unwrap_or(false)
                && item.get("status").and_then(|v| v.as_str()) == Some("proposed")
        }),
        "write gate must admit the high-signal candidate as proposed without a model"
    );
    let fact_get = run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &project_dir.to_string_lossy(),
            "--subject",
            "Cache",
            "--predicate",
            "uses",
            "--json",
        ],
        None,
    );
    assert!(fact_get.status.success());
    let value = parse_stdout_json(&fact_get);
    assert_eq!(
        value
            .get("active")
            .and_then(|entry| entry.get("value"))
            .and_then(|entry| entry.as_str()),
        Some("redis")
    );
}

#[test]
fn no_daemon_process_after_command() {
    let home = fresh_home("no-daemon");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    let maintained = run_cli(&home, &["maintain", "--budget-ms", "5000"], None);
    assert!(maintained.status.success());
    let tag = home
        .file_name()
        .and_then(|name| name.to_str())
        .expect("home tag");
    if let Ok(output) = Command::new("pgrep").arg("-f").arg(tag).output() {
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            !output.status.success(),
            "no maintain process for this run may survive after the command ends: {text}"
        );
    }
}

#[test]
fn contradiction_does_not_become_false_lesson() {
    let home = fresh_home("contradiction");
    let project_dir = home.join("repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    assert!(run_cli(&home, &["init"], None).status.success());
    let asserted = run_cli(
        &home,
        &[
            "memory",
            "assert",
            "--project",
            &project_dir.to_string_lossy(),
            "--subject",
            "cache",
            "--predicate",
            "uses",
            "--value",
            "redis",
            "--status",
            "verified",
        ],
        None,
    );
    assert!(
        asserted.status.success(),
        "assert failed: {}",
        String::from_utf8_lossy(&asserted.stderr)
    );
    let event = checkpoint_event(
        "conflict-session",
        &project_dir,
        "Session cache notes",
        "cache uses memcached for sessions",
    );
    assert!(ingest_checkpoint(&home, &event).status.success());
    let maintained = run_cli(
        &home,
        &[
            "maintain",
            "--project",
            &project_dir.to_string_lossy(),
            "--budget-ms",
            "30000",
        ],
        None,
    );
    assert!(maintained.status.success());
    let fact_get = run_cli(
        &home,
        &[
            "memory",
            "fact-get",
            "--project",
            &project_dir.to_string_lossy(),
            "--subject",
            "cache",
            "--predicate",
            "uses",
            "--json",
        ],
        None,
    );
    assert!(fact_get.status.success());
    let value = parse_stdout_json(&fact_get);
    assert_eq!(
        value
            .get("active")
            .and_then(|entry| entry.get("value"))
            .and_then(|entry| entry.as_str()),
        Some("redis"),
        "conflicting candidate must not overwrite the verified fact"
    );
    let items = review_items(&home, &project_dir);
    let false_lessons = items
        .iter()
        .filter(|item| {
            item.get("kind").and_then(|v| v.as_str()) == Some("lesson")
                && item
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|text| text.contains("memcached"))
                    .unwrap_or(false)
                && item.get("status").and_then(|v| v.as_str()) == Some("verified")
        })
        .count();
    assert_eq!(
        false_lessons, 0,
        "no verified lesson may come from a conflict"
    );
}

#[test]
fn schedule_install_status_uninstall_on_temp_home() {
    let home = fresh_home("schedule");
    assert!(run_cli(&home, &["init"], None).status.success());
    for kind in ["launchd", "systemd", "cron"] {
        let installed = run_cli(
            &home,
            &[
                "schedule",
                "install",
                "--kind",
                kind,
                "--interval-minutes",
                "30",
            ],
            None,
        );
        assert!(
            installed.status.success(),
            "install {kind} failed: {}",
            String::from_utf8_lossy(&installed.stderr)
        );
        let status = run_cli(&home, &["schedule", "status", "--kind", kind], None);
        assert!(status.status.success());
        let text = String::from_utf8_lossy(&status.stdout).to_string();
        assert!(
            text.contains("installed:"),
            "status must show installed files for {kind}: {text}"
        );
        let schedule_dir = home.join("schedule");
        assert!(
            schedule_dir.exists(),
            "schedule dir must stay inside the workspace"
        );
        let uninstalled = run_cli(&home, &["schedule", "uninstall", "--kind", kind], None);
        assert!(uninstalled.status.success());
        let after = run_cli(&home, &["schedule", "status", "--kind", kind], None);
        let after_text = String::from_utf8_lossy(&after.stdout).to_string();
        assert!(
            after_text.contains("no "),
            "schedule must be gone after uninstall for {kind}: {after_text}"
        );
    }
}
