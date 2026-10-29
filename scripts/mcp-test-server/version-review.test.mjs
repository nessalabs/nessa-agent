import assert from "node:assert/strict"
import { test } from "node:test"
import {
  parseScenario,
  runSteps,
  turnFor,
  versionReviewCall,
} from "./scripted-scenario.mjs"
import { STRESS, stressScenario } from "./scenarios/stress.mjs"

async function run(
  agent,
  outcome,
  {
    cancel = false,
    reject = false,
    executionFails = false,
    cancelledBranch = false,
    cancelAfterReply = false,
  } = {},
) {
  const calls = new Map(),
    updates = []
  let executions = 0
  const steps = turnFor(
    parseScenario(stressScenario({ agent: "codex" })),
    STRESS.listPermission,
  ).steps
  if (cancelledBranch) steps[0].on.cancelled = [{ do: "text", chunks: ["Cancelled."] }]
  let cancelled = cancel
  const result = await runSteps(steps, {
    agent,
    sessionId: "session",
    cancelled: () => cancelled,
    toolId: () => "native-call",
    messageId: () => "message",
    update: (frame) => updates.push(frame),
    rememberCall: (name, id) => calls.set(name, id),
    openCall: (name) => calls.get(name),
    clearCall: (name) => calls.delete(name),
    requestPermission: async (request) => {
      assert.equal(request.toolCall.rawInput.command, "node --version")
      if (reject) throw new Error("request failed")
      if (cancelAfterReply) cancelled = true
      return { outcome }
    },
    runVersion: async () => {
      executions++
      if (executionFails) throw new Error("native command failed")
      return { content: [{ type: "text", text: "v24.0.0" }] }
    },
    callTool: () => assert.fail("a native command must not be an MCP call"),
  })
  return { result, updates, executions, calls }
}
test("native compact fixture keeps the actual command, without an MCP wrapper", () => {
  for (const agent of ["codex"]) {
    const call = versionReviewCall(agent, "native-call")
    assert.equal(call.update.kind, "execute")
    assert.deepEqual(call.update.rawInput, { command: "node --version" })
    assert.equal(call.update._meta?.is_mcp_tool_call, undefined)
    assert.equal(
      call.update._meta?.claudeCode?.toolName,
      agent === "claude" ? "Bash" : undefined,
    )
    const { sessionUpdate, ...toolCall } = call.update
    assert.equal(sessionUpdate, "tool_call")
    assert.deepEqual(call.request("session").toolCall, toolCall)
  }
})
test("only an acknowledged allow executes and completes the same native call", async () => {
  for (const agent of ["codex"]) {
    const r = await run(agent, { outcome: "selected", optionId: "allow-once" })
    assert.equal(r.executions, 1)
    assert.equal(r.result.stopReason, "end_turn")
    assert.equal(r.calls.size, 0)
    const completed = r.updates.filter((f) => f.sessionUpdate === "tool_call_update")
    assert.equal(completed.length, 1)
    assert.equal(completed[0].toolCallId, "native-call")
    assert.equal(completed[0].status, "completed")
    assert.equal(completed[0].content[0].content.text, "v24.0.0")
  }
})
test("denial, cancellation and refusal do not execute the native command", async () => {
  for (const agent of ["codex"]) {
    for (const [outcome, options] of [
      [{ outcome: "selected", optionId: "deny-once" }, {}],
      [{ outcome: "cancelled" }, {}],
      [null, { reject: true }],
      [null, { cancel: true }],
    ]) {
      const r = await run(agent, outcome, options)
      assert.equal(r.executions, 0)
      assert.equal(
        r.updates.some((f) => f.status === "completed"),
        false,
      )
    }
  }
})
test("native failure is not completed or followed by successful branch output", async () => {
  const r = await run(
    "codex",
    { outcome: "selected", optionId: "allow-once" },
    { executionFails: true },
  )
  assert.equal(r.executions, 1)
  assert.equal(r.result.fail, "native command failed")
  assert.equal(r.updates.at(-1).status, "failed")
  assert.equal(
    r.updates.some((f) => f.sessionUpdate === "agent_message_chunk"),
    false,
  )
})
test("a cancelled outcome carrying an allow id still cannot execute", async () => {
  for (const agent of ["codex"]) {
    const r = await run(
      agent,
      { outcome: "cancelled", optionId: "allow-once" },
      { cancelledBranch: true },
    )
    assert.equal(r.executions, 0)
    assert.equal(
      r.updates.some((frame) => frame.status === "completed"),
      false,
    )
    assert.equal(r.updates.at(-1).status, "failed")
  }
})
test("a late allow after caller cancellation and an unknown selection do not execute", async () => {
  for (const options of [{ cancelAfterReply: true }, {}]) {
    const r = await run(
      "codex",
      {
        outcome: "selected",
        optionId: options.cancelAfterReply ? "allow-once" : "unknown",
      },
      options,
    )
    assert.equal(r.executions, 0)
    assert.equal(
      r.updates.some((frame) => frame.status === "completed"),
      false,
    )
  }
})
test("a scenario cannot substitute an arbitrary command into this fixed fixture", () => {
  assert.throws(
    () =>
      parseScenario({
        turns: [
          {
            steps: [
              {
                do: "version-review",
                command: "touch unexpected",
                on: { "allow-once": [{ do: "end" }] },
              },
            ],
          },
        ],
      }),
    /unknown version-review field command/,
  )
})

test("compact fixtures respect each harness's reviewable request shape", () => {
  const claude = turnFor(
    parseScenario(stressScenario({ agent: "claude" })),
    STRESS.listPermission,
  ).steps[0]
  assert.equal(claude.do, "permission")
  assert.equal(claude.tool, "report_rows")
  assert.deepEqual(claude.arguments, {})
  const codex = turnFor(
    parseScenario(stressScenario({ agent: "codex" })),
    STRESS.listPermission,
  ).steps[0]
  assert.equal(codex.do, "version-review")
  assert.throws(() => versionReviewCall("claude", "call"), /requires Codex/)
  assert.throws(
    () => stressScenario({ agent: "unknown" }),
    /unsupported stress fixture agent/,
  )
})
