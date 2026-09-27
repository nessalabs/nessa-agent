#!/usr/bin/env node
/**
 * Deterministic Claude ACP boundary for the native-window smoke test.
 *
 * The gateway and SDK orchestration stay real. This process replaces only the
 * external coding agent, echoes the submitted text, and advertises the exact
 * Claude ACP profile the production adapter validates.
 */
import { writeFileSync } from "node:fs"
import { isAbsolute } from "node:path"
import { createInterface } from "node:readline"

writeFileSync("native-smoke-provider.pid", `${process.pid}\n`, { mode: 0o600 })

const sessionId = `native-smoke-${process.pid}`
const model = process.env.ANTHROPIC_MODEL
if (typeof model !== "string" || model.length === 0)
  throw new Error("ANTHROPIC_MODEL did not name a model")
if (process.env.ANTHROPIC_CUSTOM_MODEL_OPTION !== model)
  throw new Error("Claude model environment disagrees")
if (!/^\d+$/.test(process.env.CLAUDE_CODE_MAX_OUTPUT_TOKENS ?? ""))
  throw new Error("Claude output limit is missing")

function send(value) {
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", ...value })}\n`)
}

function options() {
  return {
    configOptions: [
      {
        id: "model",
        currentValue: model,
      },
      { id: "mode", currentValue: "default" },
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
          version: "0.76.0",
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
  if (method === "session/new") {
    if (!isAbsolute(message.params?.cwd ?? ""))
      throw new Error("Claude session workspace was not absolute")
    if (message.params?._meta?.claudeCode?.options?.model !== model)
      throw new Error("Claude session did not select the configured model")
    send({
      id: message.id,
      result: {
        sessionId,
        ...options(),
      },
    })
    continue
  }
  if (method === "session/resume") {
    send({ id: message.id, result: options() })
    continue
  }
  if (method === "session/set_config_option") {
    if (message.params?.sessionId !== sessionId)
      throw new Error("Claude configuration named the wrong session")
    if (message.params.configId !== "mode" || message.params.value !== "default")
      throw new Error("Claude configuration did not select default mode")
    send({ id: message.id, result: options() })
    continue
  }
  if (method === "session/prompt") {
    if (message.params?.sessionId !== sessionId)
      throw new Error("Claude prompt named the wrong session")
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
