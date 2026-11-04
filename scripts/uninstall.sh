#!/bin/sh
set -eu

PREFIX="${HOME:-/tmp}/.local"
DATA_HOME=""
PURGE_DATA=0
HOSTS="auto"
INTEGRATE=1

usage() {
  echo "usage: uninstall.sh [--prefix DIR] [--home DIR] [--hosts LIST] [--keep-integrations] [--purge-data]" >&2
  echo "  --hosts LIST   auto (default), all, or a comma-separated list: codex,claude-code,opencode,hermes" >&2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --home) DATA_HOME="$2"; shift 2 ;;
    --hosts) HOSTS="$2"; shift 2 ;;
    --keep-integrations) INTEGRATE=0; shift ;;
    --purge-data) PURGE_DATA=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown arg: $1" >&2; usage; exit 2 ;;
  esac
done

if [ -z "$PREFIX" ]; then
  echo "refusing to uninstall with empty --prefix" >&2
  exit 2
fi

TARGET="$PREFIX/bin/agent-memory"
if [ -e "$TARGET" ]; then
  if [ "$INTEGRATE" -eq 1 ]; then
    echo "removing agent host integrations"
    if [ -n "$DATA_HOME" ]; then
      if "$TARGET" --home "$DATA_HOME" integration uninstall "$HOSTS"; then :; else
        echo "warning: host integration cleanup failed; retry with: $TARGET integration uninstall $HOSTS" >&2
      fi
    else
      if "$TARGET" integration uninstall "$HOSTS"; then :; else
        echo "warning: host integration cleanup failed; retry with: $TARGET integration uninstall $HOSTS" >&2
      fi
    fi
  else
    echo "kept host integrations (--keep-integrations)"
  fi
  rm -f "$TARGET"
  echo "removed $TARGET"
else
  echo "not installed at $TARGET (nothing to remove)"
fi

if [ "$PURGE_DATA" -eq 1 ]; then
  if [ -z "$DATA_HOME" ]; then
    if [ -n "${AGENT_MEMORY_HOME:-}" ]; then
      DATA_HOME="$AGENT_MEMORY_HOME"
    else
      echo "no data directory given: pass --home or set AGENT_MEMORY_HOME to purge" >&2
      exit 2
    fi
  fi
  rm -rf "$DATA_HOME"
  echo "purged data directory $DATA_HOME"
else
  echo "data directory preserved by default (pass --purge-data to remove it)"
fi
