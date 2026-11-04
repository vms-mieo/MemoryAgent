from __future__ import annotations

import json
import logging
import os
import shutil
import subprocess
import threading
from pathlib import Path
from typing import Any, Dict, List

from agent.memory_provider import MemoryProvider
from tools.registry import tool_error

logger = logging.getLogger(__name__)

_BRIDGE_CONFIG_NAME = "agent-memory.json"
_HOOK_TIMEOUT_MS = 4000
_TOOL_TIMEOUT_SECS = 20

SEARCH_SCHEMA = {
    "name": "agent_memory_search",
    "description": (
        "Search local agent memory for past decisions, lessons, and facts. "
        "Use before starting unfamiliar work or when a past discussion may have settled the approach."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "What to look for."},
            "top_k": {"type": "integer", "description": "Max results (default: 5, max: 20)."},
        },
        "required": ["query"],
    },
}

CONTEXT_SCHEMA = {
    "name": "agent_memory_context",
    "description": (
        "Get a compact memory briefing for the current task, with sources. "
        "Use when resuming work or needing project conventions and open blockers."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "task": {"type": "string", "description": "Current task or question."},
        },
        "required": ["task"],
    },
}

REMEMBER_SCHEMA = {
    "name": "agent_memory_remember",
    "description": (
        "Store a durable fact, decision, or lesson in local agent memory. "
        "Use for explicit user preferences, confirmed fixes, and architecture decisions."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "content": {"type": "string", "description": "The fact to remember."},
            "kind": {
                "type": "string",
                "description": "Memory kind (default: fact).",
                "enum": [
                    "fact",
                    "lesson",
                    "architecture_decision",
                    "convention",
                    "incident",
                    "task",
                    "note",
                    "episode",
                ],
            },
        },
        "required": ["content"],
    },
}


def _resolve_bridge_paths(hermes_home: str) -> Dict[str, str]:
    bridge_path = Path(hermes_home) / _BRIDGE_CONFIG_NAME if hermes_home else None
    binary = os.environ.get("AGENT_MEMORY_BIN", "").strip()
    data_home = os.environ.get("AGENT_MEMORY_HOME", "").strip()
    if bridge_path is not None and bridge_path.exists():
        try:
            raw = json.loads(bridge_path.read_text(encoding="utf-8"))
            if isinstance(raw, dict):
                if not binary and isinstance(raw.get("binary"), str):
                    binary = raw["binary"].strip()
                if not data_home and isinstance(raw.get("data_home"), str):
                    data_home = raw["data_home"].strip()
        except Exception as exc:
            logger.debug("agent-memory bridge config unreadable: %s", exc)
    if not binary:
        found = shutil.which("agent-memory")
        if found:
            binary = found
    return {"binary": binary, "data_home": data_home}


def _run_cli(binary: str, data_home: str, argv: List[str], stdin_text: str, timeout: float) -> str:
    cmd = [binary]
    if data_home:
        cmd += ["--home", data_home]
    cmd += argv
    try:
        proc = subprocess.run(
            cmd,
            input=stdin_text,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except FileNotFoundError:
        raise RuntimeError(f"agent-memory binary not found: {binary}")
    except subprocess.TimeoutExpired:
        raise RuntimeError("agent-memory call timed out")
    if proc.returncode != 0:
        detail = (proc.stderr or "").strip()[:300]
        raise RuntimeError(f"agent-memory exited {proc.returncode}: {detail}")
    return proc.stdout or ""


class AgentMemoryProvider(MemoryProvider):
    def __init__(self) -> None:
        self._binary = ""
        self._data_home = ""
        self._session_id = ""
        self._hermes_home = ""
        self._project_dir = ""
        self._threads: List[threading.Thread] = []
        self._lock = threading.Lock()

    @property
    def name(self) -> str:
        return "agent-memory"

    def is_available(self) -> bool:
        paths = _resolve_bridge_paths(self._hermes_home)
        binary = paths["binary"]
        if not binary:
            return False
        candidate = Path(binary)
        if candidate.is_absolute():
            return candidate.exists() and os.access(candidate, os.X_OK)
        return shutil.which(binary) is not None

    def save_config(self, values: Dict[str, Any], hermes_home: str) -> None:
        target = Path(hermes_home) / _BRIDGE_CONFIG_NAME
        existing: Dict[str, Any] = {}
        if target.exists():
            try:
                loaded = json.loads(target.read_text(encoding="utf-8"))
                if isinstance(loaded, dict):
                    existing = loaded
            except Exception:
                existing = {}
        if isinstance(values.get("binary"), str) and values["binary"].strip():
            existing["binary"] = values["binary"].strip()
        if isinstance(values.get("data_home"), str) and values["data_home"].strip():
            existing["data_home"] = values["data_home"].strip()
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(existing, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        try:
            os.chmod(target, 0o600)
        except OSError:
            pass

    def get_config_schema(self) -> List[Dict[str, Any]]:
        return [
            {"key": "binary", "description": "Path to the agent-memory binary", "default": "agent-memory"},
            {"key": "data_home", "description": "Agent memory data home (empty = default)", "default": ""},
        ]

    def initialize(self, session_id: str, **kwargs: Any) -> None:
        self._session_id = session_id or ""
        raw_home = kwargs.get("hermes_home") or os.environ.get("HERMES_HOME", "")
        if not raw_home:
            try:
                from hermes_constants import get_hermes_home
                raw_home = str(get_hermes_home())
            except Exception:
                raw_home = str(Path.home() / ".hermes")
        self._hermes_home = str(raw_home)
        paths = _resolve_bridge_paths(self._hermes_home)
        self._binary = paths["binary"]
        self._data_home = paths["data_home"]
        try:
            self._project_dir = os.getcwd()
        except OSError:
            self._project_dir = ""
        if not self._binary:
            logger.warning("agent-memory binary not resolved; provider idle")

    def _hook_envelope(self, event: str, extra: Dict[str, Any]) -> str:
        return json.dumps(
            {
                "hook_event_name": event,
                "session_id": self._session_id,
                "cwd": self._project_dir,
                "extra": extra,
            },
            ensure_ascii=False,
        )

    def _hook_context(self, event: str, extra: Dict[str, Any]) -> str:
        if not self._binary:
            return ""
        try:
            stdout = _run_cli(
                self._binary,
                self._data_home,
                ["hook", "context", "--host", "hermes", "--event", event, "--timeout-ms", str(_HOOK_TIMEOUT_MS)],
                self._hook_envelope(event, extra),
                timeout=_HOOK_TIMEOUT_MS / 1000 + 5,
            )
        except Exception as exc:
            logger.debug("agent-memory hook context failed: %s", exc)
            return ""
        try:
            payload = json.loads(stdout)
        except Exception:
            return ""
        if isinstance(payload, dict):
            text = payload.get("context")
            if isinstance(text, str) and text.strip():
                return text.strip()[:8000]
        return ""

    def _hook_ingest(self, event: str, extra: Dict[str, Any]) -> None:
        if not self._binary:
            return
        try:
            _run_cli(
                self._binary,
                self._data_home,
                ["hook", "ingest", "--host", "hermes", "--event", event],
                self._hook_envelope(event, extra),
                timeout=15,
            )
        except Exception as exc:
            logger.debug("agent-memory hook ingest failed: %s", exc)

    def _spawn(self, target, *args: Any) -> None:
        thread = threading.Thread(target=target, args=args, daemon=True)
        with self._lock:
            self._threads = [t for t in self._threads if t.is_alive()]
            self._threads.append(thread)
        thread.start()

    def system_prompt_block(self) -> str:
        if not self._binary:
            return ""
        return (
            "# Agent Memory\n"
            "Local project memory is active. "
            "Use agent_memory_search before unfamiliar work, "
            "agent_memory_context when resuming, and "
            "agent_memory_remember for durable facts the user confirms."
        )

    def prefetch(self, query: str, *, session_id: str = "") -> str:
        text = self._hook_context("pre_llm_call", {"user_message": query or ""})
        if not text:
            return ""
        return "## Agent Memory\n" + text

    def queue_prefetch(self, query: str, *, session_id: str = "") -> None:
        return None

    def sync_turn(
        self,
        user_content: str,
        assistant_content: str,
        *,
        session_id: str = "",
        messages: Any = None,
    ) -> None:
        self._spawn(
            self._hook_ingest,
            "post_llm_call",
            {"user_message": user_content or "", "assistant_response": assistant_content or ""},
        )

    def get_tool_schemas(self) -> List[Dict[str, Any]]:
        return [SEARCH_SCHEMA, CONTEXT_SCHEMA, REMEMBER_SCHEMA]

    def _project_flag(self) -> List[str]:
        if self._project_dir:
            return ["--project", self._project_dir]
        return ["--project", "."]

    def handle_tool_call(self, tool_name: str, args: Dict[str, Any], **kwargs: Any) -> str:
        if not self._binary:
            return tool_error("agent-memory binary is not configured")
        if not isinstance(args, dict):
            args = {}
        try:
            if tool_name == "agent_memory_search":
                query = str(args.get("query") or "").strip()
                if not query:
                    return tool_error("query is required")
                try:
                    top_k = max(1, min(20, int(args.get("top_k", 5))))
                except (TypeError, ValueError):
                    top_k = 5
                stdout = _run_cli(
                    self._binary,
                    self._data_home,
                    ["memory", "search", *self._project_flag(), "--query", query,
                     "--limit", str(top_k), "--json"],
                    "",
                    timeout=_TOOL_TIMEOUT_SECS,
                )
                return stdout
            if tool_name == "agent_memory_context":
                task = str(args.get("task") or "").strip()
                if not task:
                    return tool_error("task is required")
                stdout = _run_cli(
                    self._binary,
                    self._data_home,
                    ["memory", "context", *self._project_flag(), "--task", task,
                     "--token-budget", "400", "--json"],
                    "",
                    timeout=_TOOL_TIMEOUT_SECS,
                )
                return stdout
            if tool_name == "agent_memory_remember":
                content = str(args.get("content") or "").strip()
                if not content:
                    return tool_error("content is required")
                kind = str(args.get("kind") or "fact").strip() or "fact"
                stdout = _run_cli(
                    self._binary,
                    self._data_home,
                    ["memory", "add", *self._project_flag(), "--kind", kind,
                     "--text", content, "--scope", "project", "--actor", "hermes", "--json"],
                    "",
                    timeout=_TOOL_TIMEOUT_SECS,
                )
                return stdout
        except Exception as exc:
            logger.debug("agent-memory tool %s failed: %s", tool_name, exc)
            return tool_error(str(exc))
        return tool_error(f"Unknown tool: {tool_name}")

    def on_memory_write(
        self,
        action: str,
        target: str,
        content: str,
        metadata: Any = None,
    ) -> None:
        if action != "add" or not content or not content.strip() or not self._binary:
            return
        kind = "fact" if target == "user" else "note"
        self._spawn(self._remember_text, content.strip(), kind)

    def _remember_text(self, content: str, kind: str) -> None:
        try:
            _run_cli(
                self._binary,
                self._data_home,
                ["memory", "add", *self._project_flag(), "--kind", kind,
                 "--text", content, "--scope", "project", "--actor", "hermes", "--json"],
                "",
                timeout=_TOOL_TIMEOUT_SECS,
            )
        except Exception as exc:
            logger.debug("agent-memory mirror write failed: %s", exc)

    def shutdown(self) -> None:
        with self._lock:
            threads = list(self._threads)
        for thread in threads:
            thread.join(timeout=5.0)


def register(ctx) -> None:
    try:
        ctx.register_memory_provider(AgentMemoryProvider())
    except Exception as exc:
        logger.warning("agent-memory provider registration failed: %s", exc)
