import assert from "node:assert/strict"
import { test } from "node:test"
import { parentRestarted, parentStopCheckpoint } from "./stress-parent-stop.mjs"
import { record } from "../../../../scripts/mcp-test-server/acp-recorder.mjs"

const prompt = (sessionId, text) =>
  JSON.stringify({
    direction: "to-agent",
    frame: {
      jsonrpc: "2.0",
      method: "session/prompt",
      params: { sessionId, prompt: [{ type: "text", text }] },
    },
  }) + "\n"
const text = "stress parent stop r14 s1"
const recording = prompt("earlier-engine", text) + prompt("parent", text)
const checkpoint = parentStopCheckpoint(recording, text)
const before = new Set(["selected"])
const withdrawn = [{ executionId: "queued", status: "cancelled", parts: [] }]

test("withdrawn queued history is not provider work", () => {
  assert.equal(parentRestarted(checkpoint, recording, before, withdrawn), false)
})

test("a new prompt restarts the selected parent even if cancelled without output", () => {
  assert.equal(
    parentRestarted(
      checkpoint,
      recording + prompt("parent", "child cancellation report"),
      before,
      withdrawn,
    ),
    true,
  )
})

test("another native session's work does not restart this parent", () => {
  assert.equal(
    parentRestarted(
      checkpoint,
      recording + prompt("earlier-engine", "other work"),
      before,
      withdrawn,
    ),
    false,
  )
})

test("new live, completed, or output-bearing history fails even without dispatch evidence", () => {
  for (const turn of [
    { status: "running", parts: [] },
    { status: "completed", parts: [] },
    { status: "cancelled", parts: [{ text: "reply" }] },
  ])
    assert.equal(
      parentRestarted(checkpoint, recording, before, [{ executionId: "new", ...turn }]),
      true,
    )
})

test("missing, malformed, incomplete, or replaced recordings cannot prove no restart", () => {
  for (const invalid of ["", "{}\n", "invalid\n", recording.trimEnd()])
    assert.throws(() => parentStopCheckpoint(invalid, text))
  assert.throws(() => parentStopCheckpoint(prompt("other", "another request"), text))
  assert.throws(() => parentRestarted(checkpoint, prompt("other", text), before, []))
  assert.throws(() => parentRestarted(checkpoint, recording + "bad\n", before, []))
  for (const invalid of [
    { direction: "from-agent", text: "not a protocol frame" },
    { direction: "unknown", frame: { method: "session/prompt" } },
  ])
    assert.throws(() =>
      parentRestarted(checkpoint, recording + JSON.stringify(invalid) + "\n", before, []),
    )
  for (const frame of [
    true,
    1,
    "lost frame",
    [],
    {},
    null,
    { jsonrpc: "2.0", id: [], result: {} },
    { jsonrpc: "2.0", id: 1, result: {}, error: {} },
    { jsonrpc: "2.0", method: 1 },
    { jsonrpc: "2.0", id: 1 },
  ])
    assert.throws(() =>
      parentRestarted(
        checkpoint,
        recording + record("to-agent", JSON.stringify(frame)),
        before,
        [],
      ),
    )
  for (const sessionId of [undefined, null, "", 42, []])
    assert.throws(() =>
      parentRestarted(
        checkpoint,
        recording +
          record(
            "to-agent",
            JSON.stringify({
              jsonrpc: "2.0",
              method: "session/prompt",
              params: { sessionId },
            }),
          ),
        before,
        [],
      ),
    )
  for (const frame of [
    { jsonrpc: "2.0", method: "session/update", params: {} },
    { jsonrpc: "2.0", id: 1, result: {} },
    { jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } },
  ])
    assert.equal(
      parentRestarted(
        checkpoint,
        recording + record("from-agent", JSON.stringify(frame)),
        before,
        withdrawn,
      ),
      false,
    )
})
