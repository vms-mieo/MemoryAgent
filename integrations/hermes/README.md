# Hermes integration

Local-first project memory for Hermes via the `agent-memory` binary.

## Install

```sh
agent-memory integration install hermes
agent-memory integration status hermes
```

`agent-memory integration install` with no host (or `auto`) installs Hermes
automatically when `$HERMES_HOME`, `~/.hermes`, or a `hermes` binary on `PATH`
is found.

Without `--host-home`, the installer uses `$HERMES_HOME` or `~/.hermes`.
With `--host-home <dir>`, `<dir>` is treated as the Hermes home
(the directory containing `config.yaml`).

The installer merges three things, preserving unrelated entries:

- `mcp_servers.agent-memory` in `config.yaml` (stdio MCP server).
- `hooks` entries for `pre_llm_call`, `post_llm_call`, `post_tool_call`,
  `on_session_start`, `on_session_end`, `on_session_finalize`, `subagent_stop`.
- Memory provider plugin at `plugins/agent-memory/` plus bridge config
  at `agent-memory.json` (binary and data-home paths, mode 0600).

## Trust

Like the Codex installer, this installer never writes Hermes shell-hook
allowlist approvals. On the first run Hermes prompts for approval;
use `--accept-hooks` or approve at the TTY prompt, then verify with
`hermes hooks doctor`.

## Activate the memory provider

```sh
hermes memory setup agent-memory
```

The provider shells out to the local binary only: recall uses
`hook context`, writes use `memory add/search/context`. No pip
dependencies and no network calls.

## Uninstall

```sh
agent-memory integration uninstall hermes
```

Uninstall removes the MCP entry, our hook entries, the bridge config,
and the plugin directory (only if it still contains our provider).
Unrelated hooks, servers, and `memory_enabled` settings are preserved.
