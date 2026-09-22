#!/usr/bin/env node
/**
 * Deterministic Codex ACP boundary for the native-window smoke test.
 *
 * The gateway and SDK orchestration stay real. This process replaces only the
 * external coding agent, echoes the submitted text, and advertises the exact
 * Codex ACP profile the production adapter validates.
 */
import { writeFileSync } from "node:fs"
import { createInterface } from "node:readline"

writeFileSync("native-smoke-provider.pid", `${process.pid}\n`, { mode: 0o600 })

const sessionId = `native-smoke-${process.pid}`
const configured = JSON.parse(process.env.CODEX_CONFIG ?? "{}")
const model = configured.model
if (typeof model !== "string" || model.length === 0)
  throw new Error("CODEX_CONFIG did not name a model")

let selectedModel = "codex-default"

function send(value) {
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", ...value })}\n`)
}

function options() {
  return {
    configOptions: [
      {
        id: "model",
        currentValue: selectedModel,
        options: [{ value: "codex-default" }, { value: model }],
      },
      { id: "reasoning_effort", currentValue: "medium" },
      { id: "mode", currentValue: "read-only" },
    ],
  }
}

function promptContent(parts) {
  if (!Array.isArray(parts)) throw new Error("ACP prompt was not an array")
  const text = []
  let images = 0
  for (const part of parts) {
    if (part?.type === "text" && typeof part.text === "string") {
      text.push(part.text)
      continue
    }
    if (
      part?.type === "image" &&
      part.mimeType === "image/png" &&
      typeof part.data === "string" &&
      part.data.length > 0
    ) {
      images += 1
      continue
    }
    throw new Error(`unexpected ACP prompt part: ${JSON.stringify(part)}`)
  }
  return { text: text.join(""), images }
}

for await (const line of createInterface({ input: process.stdin })) {
  const message = JSON.parse(line)
  const method = message.method
  if (method === "initialize") {
    send({
      id: message.id,
      result: {
        protocolVersion: 1,
        agentInfo: {
          name: "@agentclientprotocol/codex-acp",
          title: "Codex",
          version: "1.12.0",
        },
        agentCapabilities: {
          promptCapabilities: { image: true },
          sessionCapabilities: { resume: {} },
        },
        _meta: { steering: { supported: true } },
      },
    })
    continue
  }
  if (method === "session/new" || method === "session/resume") {
    send({
      id: message.id,
      result: {
        ...(method === "session/new" ? { sessionId } : {}),
        ...options(),
      },
    })
    continue
  }
  if (method === "session/set_config_option") {
    if (message.params.configId === "model") selectedModel = message.params.value
    send({ id: message.id, result: options() })
    continue
  }
  if (method === "session/prompt") {
    const prompt = promptContent(message.params.prompt)
    const images = prompt.images === 0 ? "" : ` [${prompt.images} image]`
    send({
      method: "session/update",
      params: {
        sessionId,
        update: {
          sessionUpdate: "agent_message_chunk",
          content: { type: "text", text: `Smoke reply: ${prompt.text}${images}` },
        },
      },
    })
    send({ id: message.id, result: { stopReason: "end_turn" } })
    continue
  }
  if (method === "session/cancel") continue
  throw new Error(`unexpected ACP method: ${String(method)}`)
}
