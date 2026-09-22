#!/usr/bin/env node
import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import {
  appendFileSync,
  existsSync,
  readFileSync,
  renameSync,
  statSync,
  writeFileSync,
} from "node:fs"
import process from "node:process"
import { createConnection } from "node:net"
import { once } from "node:events"
import { createInterface } from "node:readline"
import { fixtureCorrelation } from "./evidence.mjs"

const [evidencePath, statePath, supervisorPortText, supervisorNonce] =
  process.argv.slice(2)
const supervisorPort = Number(supervisorPortText)
assert.ok(
  evidencePath &&
    statePath &&
    Number.isSafeInteger(supervisorPort) &&
    supervisorPort > 0 &&
    supervisorNonce,
  "fixture evidence, state, and supervisor arguments are required",
)
const model = process.env.ANTHROPIC_MODEL
assert.ok(model, "Claude model configuration is required")
assert.equal(model, process.env.ANTHROPIC_CUSTOM_MODEL_OPTION)

function record(event) {
  const line = `${JSON.stringify(event)}\n`
  const existingBytes = existsSync(evidencePath) ? statSync(evidencePath).size : 0
  assert.ok(
    existingBytes + Buffer.byteLength(line) <= 256 * 1024,
    "fixture evidence bound",
  )
  const existingEvents = existsSync(evidencePath)
    ? readFileSync(evidencePath, "utf8").split("\n").length - 1
    : 0
  assert.ok(existingEvents < 256, "fixture event count bound")
  appendFileSync(evidencePath, line)
}

function saveState(value) {
  const temporary = `${statePath}.${process.pid}.tmp`
  writeFileSync(temporary, JSON.stringify(value))
  renameSync(temporary, statePath)
}

function loadState() {
  return existsSync(statePath)
    ? JSON.parse(readFileSync(statePath, "utf8"))
    : { sessions: [] }
}

function send(value) {
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", ...value })}\n`)
}

function result(id, value) {
  send({ id, result: value })
}

function update(providerSessionId, value) {
  send({
    method: "session/update",
    params: { sessionId: providerSessionId, update: value },
  })
}

function configuration() {
  return {
    configOptions: [
      { id: "model", currentValue: model },
      { id: "mode", currentValue: "default" },
    ],
  }
}

let providerSessionId
let pendingPrompt
let pendingPermission

function requireSession(params) {
  assert.equal(params.sessionId, providerSessionId)
}

function finishPending(stopReason) {
  assert.ok(pendingPrompt, "fixture has no active prompt")
  result(pendingPrompt.requestId, { stopReason })
  record({
    type: "terminal",
    providerSessionId,
    expectedExecutionId: pendingPrompt.expectedExecutionId,
    stopReason,
  })
  pendingPrompt = undefined
  pendingPermission = undefined
}

async function receive(message) {
  const method = message.method
  const params = message.params ?? {}
  if (method === "initialize") {
    result(message.id, {
      protocolVersion: 1,
      agentInfo: { version: "0.76.0" },
      agentCapabilities: {
        promptCapabilities: { image: true },
        sessionCapabilities: { resume: {} },
      },
      _meta: { steering: { supported: true } },
    })
    return
  }
  if (method === "session/new") {
    providerSessionId = `conversation-smoke-${randomUUID()}`
    const state = loadState()
    assert.ok(state.sessions.length < 16, "fixture provider session bound")
    state.sessions.push(providerSessionId)
    saveState(state)
    record({ type: "session-new", providerSessionId, processId: process.pid })
    result(message.id, { sessionId: providerSessionId, ...configuration() })
    return
  }
  if (method === "session/resume") {
    const state = loadState()
    assert.ok(
      state.sessions.includes(params.sessionId),
      "resume must name a session previously created by this fixture",
    )
    providerSessionId = params.sessionId
    record({ type: "session-resume", providerSessionId, processId: process.pid })
    result(message.id, configuration())
    return
  }
  if (method === "session/set_config_option") {
    requireSession(params)
    assert.equal(params.configId, "mode")
    assert.equal(params.value, "default")
    result(message.id, configuration())
    return
  }
  if (method === "session/prompt") {
    requireSession(params)
    assert.equal(pendingPrompt, undefined, "fixture accepts one active prompt")
    const expectedExecutionId = fixtureCorrelation(params.prompt)
    const promptTypes = params.prompt.map((block) => block.type)
    pendingPrompt = { requestId: message.id, expectedExecutionId }
    record({ type: "prompt", providerSessionId, expectedExecutionId, promptTypes })
    update(providerSessionId, {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: `running:${expectedExecutionId}` },
    })
    if (expectedExecutionId === "execution-answer") {
      const permissionRequestId = `review-${expectedExecutionId}`
      const toolCallId = `tool-${expectedExecutionId}`
      pendingPermission = { permissionRequestId, toolCallId, expectedExecutionId }
      update(providerSessionId, {
        sessionUpdate: "tool_call",
        toolCallId,
        title: "Inspect fixture",
        kind: "read",
        status: "pending",
        rawInput: { target: "fixture-input" },
        _meta: { claudeCode: { toolName: "Read" } },
      })
      send({
        id: permissionRequestId,
        method: "session/request_permission",
        params: {
          sessionId: providerSessionId,
          toolCall: {
            toolCallId,
            kind: "read",
            status: "pending",
            rawInput: { target: "fixture-input" },
            _meta: { claudeCode: { toolName: "Read" } },
          },
          options: [
            { optionId: "allow-once", kind: "allow_once", name: "Allow once" },
            { optionId: "deny-once", kind: "reject_once", name: "Deny once" },
          ],
        },
      })
      record({
        type: "permission-request",
        providerSessionId,
        expectedExecutionId,
        permissionRequestId,
        toolCallId,
      })
      return
    }
    if (expectedExecutionId === "execution-cancel") return
    finishPending("end_turn")
    return
  }
  if (method === "_session/steering") {
    requireSession(params)
    assert.ok(pendingPrompt, "steering must target active provider work")
    const expectedExecutionId = fixtureCorrelation(params.prompt)
    record({
      type: "steer",
      providerSessionId,
      expectedExecutionId,
      activeExpectedExecutionId: pendingPrompt.expectedExecutionId,
    })
    result(message.id, { outcome: "injected" })
    update(providerSessionId, {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: `steered:${expectedExecutionId}` },
    })
    return
  }
  if (method === "session/cancel") {
    requireSession(params)
    assert.ok(pendingPrompt, "cancellation must target active provider work")
    record({
      type: "cancel",
      providerSessionId,
      expectedExecutionId: pendingPrompt.expectedExecutionId,
    })
    finishPending("cancelled")
    return
  }
  if (pendingPermission && message.id === pendingPermission.permissionRequestId) {
    const outcome = message.result?.outcome
    assert.deepEqual(outcome, { outcome: "selected", optionId: "allow-once" })
    record({
      type: "permission-answer",
      providerSessionId,
      expectedExecutionId: pendingPermission.expectedExecutionId,
      permissionRequestId: pendingPermission.permissionRequestId,
      optionId: outcome.optionId,
    })
    update(providerSessionId, {
      sessionUpdate: "tool_call_update",
      toolCallId: pendingPermission.toolCallId,
      status: "completed",
      content: [],
    })
    finishPending("end_turn")
    return
  }
  throw new Error(`unexpected ACP message: ${JSON.stringify(message)}`)
}

const supervisor = createConnection(supervisorPort, "127.0.0.1")
await once(supervisor, "connect")
supervisor.write(
  `${JSON.stringify({ nonce: supervisorNonce, processId: process.pid })}\n`,
)
let finishControl
const controlFinished = new Promise((resolve) => {
  finishControl = resolve
})
let controlInput = ""
supervisor.on("data", (bytes) => {
  controlInput += bytes.toString()
  if (controlInput.length > 64) {
    finishControl(new Error("fixture supervisor command exceeds its bound"))
    return
  }
  if (!controlInput.includes("\n")) return
  finishControl(
    controlInput === "shutdown\n"
      ? undefined
      : new Error("unexpected fixture supervisor command"),
  )
})
supervisor.on("error", (error) => finishControl(error))
supervisor.on("close", () =>
  finishControl(new Error("fixture supervisor connection closed")),
)

record({ type: "process-start", processId: process.pid })
const lines = createInterface({ input: process.stdin, crlfDelay: Infinity })
let fixtureFailure
const consumeInput = async () => {
  for await (const line of lines) {
    await receive(JSON.parse(line))
  }
}
try {
  const controlFailure = await Promise.race([
    consumeInput().then(() => undefined),
    controlFinished,
  ])
  if (controlFailure) throw controlFailure
} catch (error) {
  fixtureFailure = error
  record({ type: "fixture-failure", message: error?.message ?? String(error) })
} finally {
  lines.close()
  process.stdin.destroy()
  record({ type: "process-end", processId: process.pid, providerSessionId })
}
if (fixtureFailure) process.stderr.write(`${fixtureFailure.stack ?? fixtureFailure}\n`)
process.exit(fixtureFailure ? 1 : 0)
