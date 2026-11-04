#!/bin/sh
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/agent-memory"
HOME_TMP="${LAM_PROFILE_HOME:-$(mktemp -d /tmp/lam-profile-XXXXXX)}"
REPO="$HOME_TMP/repo"
mkdir -p "$REPO"

export AGENT_MEMORY_HOME="$HOME_TMP"
"$BIN" init >/dev/null

py() { python3 -c "$1"; }

echo "== startup: 20x status =="
START=$(date +%s%N)
i=0
while [ "$i" -lt 20 ]; do "$BIN" status >/dev/null; i=$((i+1)); done
END=$(date +%s%N)
py "ms=($END-$START)/1e6/20; print(f'status mean: {ms:.1f} ms over 20 runs')"

echo "== seed 2000 memories =="
START=$(date +%s%N)
i=0
while [ "$i" -lt 2000 ]; do
  "$BIN" memory add --project "$REPO" --kind note --text "Profile note number $i about latency budgeting" >/dev/null
  i=$((i+1))
done
END=$(date +%s%N)
py "ms=($END-$START)/1e6/2000; print(f'add mean: {ms:.1f} ms per memory over 2000')"

echo "== search p50/p95 over 50 queries =="
py "
import json, subprocess, time
lat=[]
for q in ['latency budgeting','profile note','number 1999','budgeting latency profile','missing-term-xyz']:
    for _ in range(10):
        t=time.perf_counter()
        subprocess.run(['$BIN','--home','$HOME_TMP','memory','search','--project','$REPO','--query',q,'--include-history','--json'],capture_output=True,check=True)
        lat.append((time.perf_counter()-t)*1000)
lat.sort()
print(f'search p50: {lat[len(lat)//2]:.1f} ms, p95: {lat[int(len(lat)*0.95)-1]:.1f} ms over {len(lat)} runs, db memories: 2000')
"

echo "== hook ingest latency =="
py "
import json, subprocess, time
event=json.dumps({'session_id':'prof-1','transcript_path':'/tmp/t.jsonl','cwd':'$REPO','hook_event_name':'PreCompact','goal':'Profile hook latency','current_task':'Measure hook ingest time','completed_steps':[],'next_steps':[],'blockers':[],'files_touched':[]}).encode()
lat=[]
for _ in range(10):
    t=time.perf_counter()
    subprocess.run(['$BIN','--home','$HOME_TMP','hook','ingest','--host','claude-code','--event','PreCompact'],input=event,capture_output=True,check=True)
    lat.append((time.perf_counter()-t)*1000)
lat.sort()
print(f'hook ingest p50: {lat[len(lat)//2]:.1f} ms, p95: {lat[-1]:.1f} ms over {len(lat)} runs')
"

echo "== context assembly =="
py "
import json, subprocess, time
lat=[]
for _ in range(10):
    t=time.perf_counter()
    subprocess.run(['$BIN','--home','$HOME_TMP','memory','context','--project','$REPO','--task','latency budgeting profile','--json'],capture_output=True,check=True)
    lat.append((time.perf_counter()-t)*1000)
lat.sort()
print(f'context p50: {lat[len(lat)//2]:.1f} ms, p95: {lat[-1]:.1f} ms over {len(lat)} runs')
"

echo "== RSS (macOS /usr/bin/time -l) =="
/usr/bin/time -l "$BIN" --home "$HOME_TMP" status 2>&1 | grep -E "maximum resident" || true

echo "== sizes =="
ls -la "$BIN" | awk '{print "binary bytes:", $5}'
du -sh "$HOME_TMP/memory.db" | awk '{print "db size:", $1}'
echo "home: $HOME_TMP"
