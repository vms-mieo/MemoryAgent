#!/bin/sh
set -eu

PREFIX="${HOME:-/tmp}/.local"
BIN_SRC=""
NO_BUILD=0
DATA_HOME=""
HOSTS="auto"
INTEGRATE=1

usage() {
  echo "usage: install.sh [--bin PATH] [--prefix DIR] [--home DIR] [--hosts LIST] [--no-integrate] [--no-build]" >&2
  echo "  --hosts LIST   auto (default), all, or a comma-separated list: codex,claude-code,opencode,hermes" >&2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN_SRC="$2"; shift 2 ;;
    --prefix) PREFIX="$2"; shift 2 ;;
    --home) DATA_HOME="$2"; shift 2 ;;
    --hosts) HOSTS="$2"; shift 2 ;;
    --no-integrate) INTEGRATE=0; shift ;;
    --no-build) NO_BUILD=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown arg: $1" >&2; usage; exit 2 ;;
  esac
done

if [ -z "$PREFIX" ]; then
  echo "refusing to install with empty --prefix" >&2
  exit 2
fi

if [ -z "$BIN_SRC" ]; then
  ROOT="$(cd "$(dirname "$0")/.." && pwd)"
  if [ "$NO_BUILD" -eq 0 ]; then
    (cd "$ROOT" && cargo build --release -p agent-memory)
  fi
  BIN_SRC="$ROOT/target/release/agent-memory"
fi

if [ ! -x "$BIN_SRC" ]; then
  echo "binary not found or not executable: $BIN_SRC" >&2
  exit 1
fi

mkdir -p "$PREFIX/bin"
TARGET="$PREFIX/bin/agent-memory"
TMP_TARGET="$TARGET.tmp.$$"
cp "$BIN_SRC" "$TMP_TARGET"
chmod 755 "$TMP_TARGET"
mv -f "$TMP_TARGET" "$TARGET"

echo "installed agent-memory to $PREFIX/bin/agent-memory"
if [ -n "$DATA_HOME" ]; then
  echo "data directory override: $DATA_HOME (export AGENT_MEMORY_HOME=$DATA_HOME)"
else
  echo "data directory: default platform location (or AGENT_MEMORY_HOME)"
fi

if [ "$INTEGRATE" -eq 1 ]; then
  if [ "$HOSTS" = "auto" ]; then
    echo "detecting agent hosts (codex, claude-code, opencode, hermes)"
  fi
  if [ -n "$DATA_HOME" ]; then
    if "$TARGET" --home "$DATA_HOME" integration install "$HOSTS"; then :; else
      echo "warning: host integration setup failed; retry with: agent-memory integration install $HOSTS" >&2
    fi
  else
    if "$TARGET" integration install "$HOSTS"; then :; else
      echo "warning: host integration setup failed; retry with: agent-memory integration install $HOSTS" >&2
    fi
  fi
else
  echo "skipped host integration setup (--no-integrate)"
  echo "next (optional): agent-memory integration install auto"
fi

echo "next: agent-memory init"
