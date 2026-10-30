import { strict as assert } from "node:assert"
import { test } from "node:test"

import { parseScenario, runSteps, turnFor } from "./scripted-scenario.mjs"
import { SAYS, STRESS, stressScenario } from "./scenarios/stress.mjs"

test("stress requests the actual harness's usage-limit frame", () => {
  const scenario = parseScenario(stressScenario())
  assert.deepEqual(turnFor(scenario, STRESS.limit).steps, [{ do: "usage-limit" }])
})

test("recursive Stop creates owned work through the selected server and waits for cancellation", async () => {
  const scenario = parseScenario(stressScenario())
  for (const [phrase, message] of [
    [STRESS.parentStop, STRESS.ownedSeed],
    [STRESS.ownedChild, STRESS.wait],
  ]) {
    const calls = []
    const chunks = []
    let waited = false
    const result = await runSteps(turnFor(scenario, phrase).steps, {
      agent: "claude",
      cancelled: () => false,
      openCall: () => null,
      clearCall: () => {},
      toolId: () => "tool",
      messageId: () => "message",
      update: (update) => {
        if (update.sessionUpdate === "agent_message_chunk")
          chunks.push(update.content.text)
      },
      callTool: async (args, server) => {
        calls.push({ args, server })
        return {
          structuredContent:
            calls.length === 1
              ? { threadId: "owned-thread" }
              : { executionId: "owned-execution" },
        }
      },
      untilCancelled: async () => {
        waited = true
      },
    })
    assert.deepEqual(
      calls.map((each) => each.server),
      ["nessa-agents", "nessa-agents"],
    )
    assert.equal(calls[0].args.name, "create_thread")
    assert.equal(calls[1].args.name, "send_message")
    assert.deepEqual(calls[1].args.arguments, { threadId: "owned-thread", message })
    assert.equal(chunks.join(""), `${SAYS.owned}owned-thread owned-execution`)
    assert.equal(waited, true)
    assert.equal(result.stopReason, "cancelled")
  }
  const waiting = turnFor(scenario, STRESS.wait)
  assert.deepEqual(waiting.steps, [
    { do: "text", chunks: [SAYS.waiting] },
    { do: "wait-cancel" },
  ])
  assert.deepEqual(
    turnFor(scenario, STRESS.ownedSeed).steps,
    [{ do: "text", chunks: ["Owned child seed completed"] }, { do: "end" }],
    "the agent-authored seed must not relay agent tools; person admission starts ownedChild",
  )
})
