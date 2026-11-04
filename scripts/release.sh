#!/bin/sh
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/dist}"
VERSION="$(grep -m1 '^version' "$ROOT/apps/agent-memory/Cargo.toml" | cut -d'"' -f2)"
TARGET_TRIPLE="$(rustc -vV | grep host | cut -d' ' -f2)"

mkdir -p "$OUT"
(cd "$ROOT" && cargo build --release -p agent-memory)

BIN="$ROOT/target/release/agent-memory"
cp "$BIN" "$OUT/agent-memory-$TARGET_TRIPLE"

if command -v shasum >/dev/null 2>&1; then
  (cd "$OUT" && shasum -a 256 "agent-memory-$TARGET_TRIPLE" > "agent-memory-$TARGET_TRIPLE.sha256")
elif command -v sha256sum >/dev/null 2>&1; then
  (cd "$OUT" && sha256sum "agent-memory-$TARGET_TRIPLE" > "agent-memory-$TARGET_TRIPLE.sha256")
else
  echo "no sha256 tool found (need shasum or sha256sum)" >&2
  exit 1
fi

cat > "$OUT/release-info.txt" <<EOF
package: agent-memory
version: $VERSION
target: $TARGET_TRIPLE
built: $(date -u +%Y-%m-%dT%H:%M:%SZ)
rustc: $(rustc -V)
source: local working tree $(cd "$ROOT" && git rev-parse --short HEAD 2>/dev/null || echo unknown)
note: local-only release artifact; this script never publishes, pushes, or tags.
EOF

echo "release artifact: $OUT/agent-memory-$TARGET_TRIPLE"
cat "$OUT/agent-memory-$TARGET_TRIPLE.sha256"
echo "this script does not publish, push, or tag any release"
