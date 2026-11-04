#!/bin/sh
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/agent-memory"
WORK="$(mktemp -d /tmp/lam-final-demo-XXXXXX)"
REPO="$WORK/repo"
OUT="$ROOT/docs/testing/final-demo.md"
mkdir -p "$REPO/docs" "$REPO/src"

export AGENT_MEMORY_HOME="$WORK"
export http_proxy="http://127.0.0.1:9"
export https_proxy="http://127.0.0.1:9"
export all_proxy="http://127.0.0.1:9"

{
echo "# Final demonstration transcript (§25)"
echo
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Binary: release \`agent-memory\` built from this tree"
echo "Home: hermetic temp dir (redacted); proxy env poisoned to prove offline operation"
echo
} > "$OUT"

run() {
  echo "\$ $*" >> "$OUT"
  # shellcheck disable=SC2068
  out="$("$@" 2>&1)"
  echo '```' >> "$OUT"
  echo "$out" >> "$OUT"
  echo '```' >> "$OUT"
  echo >> "$OUT"
  printf '%s\n' "$out"
}

need() {
  if ! printf '%s' "$1" | grep -q "$2"; then
    echo "demo assertion failed: missing [$2]" >&2
    exit 1
  fi
}

echo "## Setup" >> "$OUT"
run "$BIN" init
printf '# Provisioning\n\nADR-0001 accepted: use Temporal for orchestration.\n' > "$REPO/docs/adr-0001.md"
printf 'fn main() {}\n' > "$REPO/src/main.rs"

echo "## Run 1 (Claude Code): design provisioning, accept Temporal ADR" >> "$OUT"
run "$BIN" project bind --path "$REPO" --name demo-repo
EV1="{\"session_id\":\"claude-run-1\",\"transcript_path\":\"/tmp/t.jsonl\",\"cwd\":\"$REPO\",\"hook_event_name\":\"PreCompact\",\"goal\":\"Provision database with Temporal workflow\",\"current_task\":\"Wire worker deployment config\",\"completed_steps\":[\"ADR-0001 accepted\",\"worker code generated\"],\"next_steps\":[\"Apply migration\",\"Run smoke test\"],\"blockers\":[\"Waiting on staging credentials\"],\"files_touched\":[\"deploy/worker.yaml\",\"docs/adr-0001.md\"]}"
echo "$EV1" | run "$BIN" hook ingest --host claude-code --event PreCompact
run "$BIN" memory assert --project "$REPO" --subject database.engine --predicate uses --value Temporal --status verified --reason "ADR-0001"

echo "## Run 2 (Codex, brand-new session): recall ADR, blockers, next actions" >> "$OUT"
EV2="{\"session_id\":\"codex-run-2\",\"transcript_path\":\"/tmp/t.jsonl\",\"cwd\":\"$REPO\",\"hook_event_name\":\"SessionStart\"}"
CTX="$(echo "$EV2" | "$BIN" hook context --host codex --event SessionStart)"
{
echo "\$ hook context --host codex --event SessionStart (new session)"
echo '```'
echo "$CTX"
echo '```'
echo >> "$OUT"
} >> "$OUT"
printf '%s\n' "$CTX"
need "$CTX" "Temporal"
need "$CTX" "staging credentials"

echo "## Run 3 (OpenCode): why Temporal, then change the decision with history kept" >> "$OUT"
run "$BIN" memory search --project "$REPO" --query "Temporal orchestration" --json
run "$BIN" memory assert --project "$REPO" --subject database.engine --predicate uses --value PostgreSQL --status verified --reason "cost review switches engine"
run "$BIN" memory fact-get --project "$REPO" --subject database.engine --predicate uses --json
run "$BIN" memory timeline --project "$REPO" --subject database.engine --predicate uses --json

echo "## Run 4 (maintenance and dreaming): similar debug episodes become one proposed lesson" >> "$OUT"
i=0
while [ "$i" -lt 10 ]; do
  EV="{\"session_id\":\"debug-$((i % 3))\",\"transcript_path\":\"/tmp/t.jsonl\",\"cwd\":\"$REPO\",\"hook_event_name\":\"PreCompact\",\"goal\":\"Stabilize timeout handling in media path\",\"current_task\":\"Investigate timeout handling in media path batch $i\",\"completed_steps\":[],\"next_steps\":[],\"blockers\":[],\"files_touched\":[]}"
  echo "$EV" | "$BIN" hook ingest --host claude-code --event PreCompact >/dev/null
  i=$((i+1))
done
run "$BIN" maintain --project "$REPO" --budget-ms 30000 --json
LESSON_JSON="$(run "$BIN" memory review --list --project "$REPO" --json)"
need "$LESSON_JSON" "Recurring theme"
LESSON_ID="$(printf '%s' "$LESSON_JSON" | python3 -c "import json,sys; print([i['memory_id'] for i in json.load(sys.stdin) if 'Recurring theme' in i.get('content','')][0])")"
run "$BIN" memory approve "$LESSON_ID" --reason "confirmed on staging"
run "$BIN" memory search --project "$REPO" --query "timeout handling media" --json

echo "## Run 5 (offline privacy): poisoned network, secrets, backup and restore" >> "$OUT"
EV3="{\"session_id\":\"sec-1\",\"transcript_path\":\"/tmp/t.jsonl\",\"cwd\":\"$REPO\",\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"Deploy with token ghp_fixture0000000000000000000000 and api_key=fixture-secret-value then continue\"}"
echo "$EV3" | run "$BIN" hook ingest --host claude-code --event UserPromptSubmit
run "$BIN" privacy audit
run "$BIN" backup create --json
BK="$(ls "$WORK"/backups/*.db | head -1)"
run "$BIN" backup restore --input "$BK" --force
run "$BIN" doctor

echo "Demo complete on $WORK" >> "$OUT"
echo DEMO_OK
