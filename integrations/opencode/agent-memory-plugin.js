const LAM_BIN = __LAM_BIN__;
const LAM_HOME = __LAM_DATA_HOME__;
const HOOK_TIMEOUT_MS = 1500;
const RECALL_THROTTLE_MS = 60000;
const MAX_PART_CHARS = 8000;

import { execFileSync } from "node:child_process";

function pickSession(candidates) {
  for (const candidate of candidates) {
    if (typeof candidate === "string" && candidate.trim() !== "") {
      return candidate.trim();
    }
  }
  return "";
}

function safeString(value, limit) {
  if (typeof value !== "string") {
    return "";
  }
  const trimmed = value.trim();
  if (trimmed === "") {
    return "";
  }
  return trimmed.length > limit ? trimmed.slice(0, limit) : trimmed;
}

function runHook(sub, event, envelope) {
  try {
    const argv = [LAM_BIN, "--home", LAM_HOME, "hook", sub, "--host", "opencode", "--event", event];
    const stdout = execFileSync(argv[0], argv.slice(1), {
      input: JSON.stringify(envelope),
      timeout: HOOK_TIMEOUT_MS,
      stdio: ["pipe", "pipe", "ignore"],
      encoding: "utf8",
      maxBuffer: 65536,
    });
    return (stdout || "").trim();
  } catch {
    return "";
  }
}

function parseContextText(stdout) {
  const text = (stdout || "").trim();
  if (text === "") {
    return "";
  }
  try {
    const parsed = JSON.parse(text);
    if (parsed && typeof parsed.context_text === "string") {
      return parsed.context_text.trim().slice(0, MAX_PART_CHARS);
    }
  } catch {
  }
  return text.slice(0, MAX_PART_CHARS);
}

function injectText(output, text) {
  const clean = (text || "").trim();
  if (clean === "") {
    return;
  }
  try {
    const parts = output && output.parts;
    if (Array.isArray(parts)) {
      for (let index = parts.length - 1; index >= 0; index -= 1) {
        const part = parts[index];
        if (part && part.type === "text" && typeof part.text === "string") {
          part.text = part.text + "\n\n" + clean;
          return;
        }
      }
      parts.push({ type: "text", text: clean });
    }
  } catch {
  }
}

function isOwnMemoryTool(tool) {
  const name = String(tool || "").toLowerCase();
  return name.indexOf("memory") !== -1 && (name.indexOf("agent") !== -1 || name.indexOf("memory_") === 0 || name.indexOf("memory-") === 0);
}

export const AgentMemoryPlugin = async ({ directory, worktree }) => {
  const cwd = worktree || directory || process.cwd();
  let sessionId = "";
  let briefed = false;
  let briefing = "";
  let lastRecall = 0;

  function envelope(event, extra) {
    const fields = extra || {};
    return {
      host: "opencode",
      event,
      sessionId: pickSession([fields.sessionId, sessionId]),
      cwd,
      branch: fields.branch,
      timestamp: new Date().toISOString(),
      payload: fields.payload || {},
    };
  }

  function refreshBriefing() {
    const text = parseContextText(runHook("context", "session.created", envelope("session.created", {})));
    if (text !== "") {
      briefing = text;
    }
    return briefing;
  }

  return {
    "session.created": async (input) => {
      try {
        sessionId = pickSession([input && input.sessionId, input && input.sessionID, sessionId]);
        refreshBriefing();
      } catch {
      }
      return {};
    },
    "chat.message": async (input, output) => {
      try {
        const role = output && output.message && output.message.role;
        if (role && role !== "user") {
          return {};
        }
        sessionId = pickSession([
          output && output.message && output.message.sessionID,
          input && input.sessionId,
          input && input.sessionID,
          sessionId,
        ]);
        const parts = (output && output.parts) || [];
        let prompt = "";
        for (const part of parts) {
          if (part && part.type === "text" && typeof part.text === "string" && part.text.trim() !== "") {
            prompt = part.text;
            break;
          }
        }
        const now = Date.now();
        if (!briefed) {
          briefed = true;
          lastRecall = now;
          const first = briefing !== "" ? briefing : refreshBriefing();
          injectText(output, first);
          return {};
        }
        if (prompt !== "" && now - lastRecall >= RECALL_THROTTLE_MS) {
          lastRecall = now;
          const text = parseContextText(
            runHook("context", "chat.message", envelope("chat.message", { payload: { prompt: safeString(prompt, 2000) } }))
          );
          injectText(output, text);
        }
      } catch {
      }
      return {};
    },
    "tool.execute.after": async (input) => {
      try {
        const tool = (input && (input.tool || input.toolName)) || "";
        if (isOwnMemoryTool(tool)) {
          return {};
        }
        sessionId = pickSession([input && input.sessionId, input && input.sessionID, sessionId]);
        const payload = {
          tool: safeString(String(tool), 256),
          output: safeString(JSON.stringify(input && input.output).slice(0, 2000), 2000),
        };
        runHook("ingest", "tool.execute.after", envelope("tool.execute.after", { payload }));
      } catch {
      }
      return {};
    },
    "file.edited": async (input) => {
      try {
        const filePath = safeString((input && (input.file || input.filePath || input.path)) || "", 1024);
        if (filePath === "") {
          return {};
        }
        sessionId = pickSession([input && input.sessionId, input && input.sessionID, sessionId]);
        runHook("ingest", "file.edited", envelope("file.edited", { payload: { file: filePath } }));
      } catch {
      }
      return {};
    },
    "session.idle": async (input) => {
      try {
        sessionId = pickSession([input && input.sessionId, input && input.sessionID, sessionId]);
        runHook("ingest", "session.idle", envelope("session.idle", { payload: {} }));
      } catch {
      }
      return {};
    },
    "experimental.session.compacting": async (input, output) => {
      try {
        sessionId = pickSession([input && input.sessionId, input && input.sessionID, sessionId]);
        const text = parseContextText(runHook("context", "session.compacted", envelope("session.compacted", {})));
        if (text !== "" && output && Array.isArray(output.context)) {
          output.context.push(text);
        }
      } catch {
      }
      return {};
    },
  };
};
