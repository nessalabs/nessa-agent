import assert from "node:assert/strict"
import { test } from "node:test"
import {
  completedUpdate,
  permissionCall,
} from "../../../../scripts/mcp-test-server/scripted-scenario.mjs"
import { closeAcceptance, parkingEvidence, readFailureView } from "./stress-close.mjs"

const closing = "11111111-1111-4111-8111-111111111111"
const other = "22222222-2222-4222-8222-222222222222"
const text = "stress close r42 s1"
const record = (direction, method, params) =>
  JSON.stringify({ direction, frame: { jsonrpc: "2.0", method, params } }) + "\n"
const prompt = (sessionId = "close-session") =>
  record("to-agent", "session/prompt", {
    sessionId,
    prompt: [{ type: "text", text }],
  })
const update = (value, sessionId = "close-session") =>
  record("from-agent", "session/update", { sessionId, update: value })
const call = (toolCallId = "close-call") =>
  update(
    permissionCall("claude", {
      id: toolCallId,
      tool: "close_thread",
      title: "close_thread",
      arguments: {},
      server: "nessa-agents",
    }).update,
  )
const result = (value, extra = {}, sessionId) =>
  update(
    {
      sessionUpdate: "tool_call_update",
      toolCallId: "close-call",
      status: "completed",
      content: [
        { type: "content", content: { type: "text", text: JSON.stringify(value) } },
      ],
      ...extra,
    },
    sessionId,
  )
const acceptance = (receipt) =>
  closeAcceptance(prompt() + call() + receipt, text, closing)

test("Closing this thread words and a completed turn cannot accept a refused close", () => {
  const words = update({
    sessionUpdate: "agent_message_chunk",
    content: { type: "text", text: "Closing this thread: permission_denied SECRET" },
  })
  assert.equal(acceptance(result({ error: "SECRET" }) + words).state, "refused")
  assert.equal(acceptance(result(null, { status: "failed" }) + words).state, "refused")
  assert.equal(acceptance(words).state, "unconfirmed")
  assert.equal(acceptance(result({ closing: false, threadId: closing })).state, "refused")
})

test("acceptance owes the exact prompt, native session, call and conversation", () => {
  const ok = result({ threadId: closing, closing: true, secret: "SECRET" })
  assert.deepEqual(acceptance(ok), {
    state: "accepted",
    threadId: closing,
    closing: true,
  })
  assert.equal(
    acceptance(result({ threadId: other, closing: true })).state,
    "wrong-thread",
  )
  assert.equal(
    acceptance(result({ threadId: closing, closing: true }, {}, "other-session")).state,
    "unconfirmed",
  )
  assert.equal(
    acceptance(
      result({ threadId: closing, closing: true }, { toolCallId: "another-call" }),
    ).state,
    "unconfirmed",
  )
  assert.equal(
    acceptance(result({ threadId: closing, closing: true }, { status: "pending" })).state,
    "unconfirmed",
  )
  assert.equal(
    closeAcceptance(prompt() + call() + ok + prompt("new-session"), text, closing).state,
    "unconfirmed",
  )
  assert.equal(acceptance(call("second-call") + ok).state, "unconfirmed")
  const next = record("to-agent", "session/prompt", {
    sessionId: "close-session",
    prompt: [{ type: "text", text: "later turn" }],
  })
  assert.equal(acceptance(next + ok).state, "unconfirmed")
  assert.equal(
    JSON.stringify(
      acceptance(result({ threadId: "SECRET", closing: true, error: "SECRET" })),
    ).includes("SECRET"),
    false,
  )
})

test("missing or malformed tool evidence cannot pass", () => {
  assert.equal(closeAcceptance("", text, closing).state, "unconfirmed")
  assert.throws(() => closeAcceptance(prompt().trimEnd(), text, closing), /incomplete/)
  for (const value of [
    null,
    [],
    "SECRET",
    { threadId: closing },
    { closing: "true", threadId: closing },
  ])
    assert.equal(acceptance(result(value)).state, "unconfirmed")
})

test("failure view captures the fresh closing conversation instead of main", async () => {
  const run = { client: {}, main: { id: other }, failureConversationId: closing }
  const read = async (client, id) => {
    assert.equal(client, run.client)
    return { conversationId: id, lease: { state: id === closing ? "live" : "ended" } }
  }
  assert.deepEqual(await readFailureView(run, read), {
    conversationId: closing,
    lease: { state: "live" },
  })
  run.failureConversationId = null
  assert.equal((await readFailureView(run, read)).conversationId, other)
})

test("parking diagnostics distinguish target acceptance, global progress and cleanup without raw logs", () => {
  const line = (id, message) =>
    `2026-11-01T09:44:00.000Z INFO ${message} conversation_id=${id} token=SECRET\n`
  const before = line(other, "parking an idle agent")
  const log =
    before +
    line(closing, "the agent closed its thread; it stops once its turn ends") +
    line(other, "parking an idle agent") +
    line(closing, "an idle agent's stop did not confirm cleanup")
  const seen = parkingEvidence(log, before.length, closing)
  assert.equal(seen.accepted, true)
  assert.equal(seen.parking, 1)
  assert.equal(seen.targetParking, 0)
  assert.equal(seen.cleanupFailures, 1)
  assert.equal(seen.previousParking.threadId, other)
  assert.equal(seen.recent.length, 3)
  assert.equal(JSON.stringify(seen).includes("SECRET"), false)
  const parked = parkingEvidence(
    log + line(closing, "parking an idle agent"),
    before.length,
    closing,
  )
  assert.equal(parked.targetParking, 1)
  const bounded = parkingEvidence(
    line(other, "parking an idle agent").repeat(100),
    0,
    closing,
  )
  assert.equal(bounded.parking, 100)
  assert.equal(bounded.recent.length, 12)
  assert.deepEqual(parkingEvidence("token=SECRET\n", 0, closing).recent, [])
})

for (const agent of ["claude", "codex"])
  test(`${agent} scenario receipts identify the actual nessa-agents call`, () => {
    const announcement = permissionCall(agent, {
      id: "close-call",
      tool: "close_thread",
      title: "close_thread",
      arguments: {},
      server: "nessa-agents",
    }).update
    const receipt = completedUpdate("close-call", {
      content: [
        { type: "text", text: JSON.stringify({ threadId: closing, closing: true }) },
      ],
    })
    assert.equal(
      closeAcceptance(prompt() + update(announcement) + update(receipt), text, closing)
        .state,
      "accepted",
    )
    const wrong = permissionCall(agent, {
      id: "close-call",
      tool: "close_thread",
      title: "close_thread",
      arguments: {},
      server: "mcptest",
    }).update
    assert.equal(
      closeAcceptance(prompt() + update(wrong) + update(receipt), text, closing).state,
      "unconfirmed",
    )
  })

test("missing call identity cannot correlate two undefined IDs", () => {
  const declaration = permissionCall("claude", {
    id: undefined,
    tool: "close_thread",
    title: "close_thread",
    arguments: {},
    server: "nessa-agents",
  }).update
  const receipt = completedUpdate(undefined, {
    content: [
      { type: "text", text: JSON.stringify({ threadId: closing, closing: true }) },
    ],
  })
  assert.equal(
    closeAcceptance(prompt() + update(declaration) + update(receipt), text, closing)
      .state,
    "unconfirmed",
  )
})

test("receipt order and contradictory or repeated terminal outcomes cannot manufacture acceptance", () => {
  const accepted = result({ threadId: closing, closing: true })
  const refused = result(null, { status: "failed" })
  assert.equal(
    closeAcceptance(prompt() + accepted + call(), text, closing).state,
    "unconfirmed",
  )
  for (const terminals of [
    refused + accepted,
    accepted + refused,
    accepted + accepted,
    refused + refused,
  ])
    assert.equal(acceptance(terminals).state, "unconfirmed")
  assert.equal(acceptance(accepted).state, "accepted")
})
