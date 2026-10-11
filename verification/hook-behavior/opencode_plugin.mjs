// OpenCode plugin planted twice per row: the project copy and the user copy.
// The runner replaces __NESSA_HOOK_SOURCE__ with project or user before the
// process starts, because both copies share one environment. It records the
// callbacks the process actually invokes and, for one mode, mutates the object
// that callback received. OpenCode has no Claude permissionDecision vocabulary;
// those modes record the callback and leave the tool arguments alone.
import fs from "node:fs"

const logPath = process.env.NESSA_HOOK_LOG
const mode = process.env.NESSA_HOOK_MODE || "log"
const hookSource = "__NESSA_HOOK_SOURCE__"
const ORIGINAL = "hook-sentinel-original"
const REWRITTEN = "hook-sentinel-rewritten"
const CONTEXT_NONCE = "NONCE_CONTEXT_4c1e"
const SYSTEM_NONCE = "SYSMSG_7ab2"

function record(event, detail) {
  fs.appendFileSync(
    logPath,
    JSON.stringify({ event, mode, hook_source: hookSource, detail }) + "\n",
  )
}

function rewrite(value) {
  if (!value || typeof value !== "object") return
  for (const key of Object.keys(value)) {
    const item = value[key]
    if (typeof item === "string" && item.includes(ORIGINAL)) {
      value[key] = item.replaceAll(ORIGINAL, REWRITTEN)
    } else {
      rewrite(item)
    }
  }
}

export default async () => ({
  event: async ({ event }) => {
    const type = event && event.type ? event.type : "event"
    record(`event:${type}`, type)
  },
  "tool.execute.before": async (input, output) => {
    record("tool.execute.before", { tool: input.tool, args: output.args })
    if (mode === "deny") {
      throw new Error("NESSA_DENY_REASON")
    }
    if (mode === "allow-updated") rewrite(output.args)
    if (mode === "exit2" || mode === "exit1") {
      throw new Error(mode === "exit2" ? "NESSA_EXIT_2" : "NESSA_EXIT_1")
    }
    if (mode === "malformed") {
      output.args = undefined
    }
    if (mode === "timeout") {
      await new Promise((resolve) => setTimeout(resolve, 30000))
    }
  },
  "tool.execute.after": async (input, output) => {
    record("tool.execute.after", { tool: input.tool, title: output.title })
  },
  "chat.message": async (_input, output) => {
    record("chat.message", {})
    if (mode !== "context" || !output || !Array.isArray(output.parts)) return
    for (const part of output.parts) {
      if (part && typeof part.text === "string") part.text += `\n${CONTEXT_NONCE}`
    }
  },
  "experimental.chat.system.transform": async (_input, output) => {
    record("experimental.chat.system.transform", {})
    if (mode === "system" && output && Array.isArray(output.system)) {
      output.system.push(SYSTEM_NONCE)
    }
  },
  "experimental.session.compacting": async () => {
    record("experimental.session.compacting", {})
  },
  "command.execute.before": async (input) => {
    record("command.execute.before", { command: input.command })
  },
})
