import assert from "node:assert/strict"
import test from "node:test"
import { raisedTurn } from "./raise-evidence.mjs"

const line = (direction, frame) => JSON.stringify({ direction, frame })
const prompt = line("to-agent", {
  id: 18,
  method: "session/prompt",
  params: { sessionId: "s", prompt: [{ type: "text", text: "stress raise access r1" }] },
})
const ask = line("from-agent", {
  id: "perm-1",
  method: "session/request_permission",
  params: { sessionId: "s" },
})
const change = line("to-agent", {
  id: 19,
  method: "session/set_config_option",
  params: { sessionId: "s", configId: "mode", value: "bypassPermissions" },
})
const answer = line("to-agent", { id: "perm-1", result: { outcome: {} } })
const end = line("from-agent", { id: 18, result: { stopReason: "end_turn" } })
const earlier = `${line("to-agent", { id: 1, method: "initialize" })}\n`

test("a raise sent while the turn waits on its review reaches that review and the rest of the turn", () => {
  const recording = earlier + [prompt, ask, change, answer, end].join("\n")
  assert.deepEqual(raisedTurn(earlier, recording, "stress raise access r1"), {
    asked: 1,
    modeChanged: true,
    answeredAfterChange: true,
    endedAfterChange: true,
  })
})

test("a raise held until the turn ended is not seen as reaching it", () => {
  const recording = earlier + [prompt, ask, answer, end, change].join("\n")
  assert.deepEqual(raisedTurn(earlier, recording, "stress raise access r1"), {
    asked: 1,
    modeChanged: false,
    answeredAfterChange: false,
    endedAfterChange: false,
  })
})

test("frames written before the turn are not read, and an unknown prompt is null", () => {
  const recording = earlier + [prompt, ask, change, answer, end].join("\n")
  assert.equal(raisedTurn(recording, recording, "stress raise access r1"), null)
  assert.equal(raisedTurn(earlier, recording, "another prompt"), null)
})
