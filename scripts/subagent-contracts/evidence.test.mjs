import { createHash } from "node:crypto"
import { strict as assert } from "node:assert"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { EvidenceError, inspectFrames, selectFrames } from "./evidence.mjs"

const fixture = JSON.parse(
  readFileSync(new URL("./fixtures/codex-native.json", import.meta.url), "utf8"),
)
const copy = () => structuredClone(fixture.frames)
const rejects = (records, code) =>
  assert.throws(
    () => inspectFrames(records),
    (error) => error instanceof EvidenceError && error.code === code,
  )

test("live native spawn, wait and close retain one parent and one child across independent wire reports", () => {
  assert.deepEqual(inspectFrames(fixture.frames), {
    parent: "identity-1",
    child: "identity-3",
    tools: ["spawnAgent", "wait", "closeAgent"],
    activeCalls: 0,
    stopReason: "end_turn",
  })
  const complete = fixture.frames.filter(
    ({ frame }) => frame.params?.update.sessionUpdate === "tool_call_update",
  )
  assert.equal(
    complete[0].frame.params.update.rawInput.agentsStates["identity-3"].status,
    "pendingInit",
  )
  for (const { frame } of complete.slice(1))
    assert.deepEqual(frame.params.update.rawInput.agentsStates["identity-3"], {
      status: "completed",
      message: "CHILD_DONE",
    })
  assert.equal(fixture.source.initialMode, "read-only")
  assert.equal(fixture.source.modeConfig.currentValue, fixture.source.initialMode)
  assert.equal(fixture.source.permissionRequests, 0)
  assert.equal(fixture.source.nativeSessionUpdates, 0)
  assert.deepEqual(fixture.source.parentClose, { kind: "rpc_response", result: {} })
})

test("foreign sender, receiver metadata and agent state reports are refused independently", () => {
  for (const field of ["senderThreadId", "receiverThreadIds"]) {
    const records = copy()
    records[1].frame.params.update._meta.codex.collaboration[field] =
      field === "senderThreadId" ? "foreign" : ["foreign"]
    rejects(records, "identity_mismatch")
  }
  const foreign = copy()
  foreign[1].frame.params.update.rawInput.agentsStates.foreign = {
    status: "completed",
    message: "CHILD_DONE",
  }
  rejects(foreign, "foreign_agent_state")
  const changed = copy()
  changed[2].frame.params.sessionId = "foreign"
  rejects(changed, "identity_mismatch")
})

test("a structurally valid different child cannot replace the child created by spawn", () => {
  const records = copy()
  const update = records[3].frame.params.update
  update.rawInput.receiverThreadIds = ["other-child"]
  update._meta.codex.collaboration.receiverThreadIds = ["other-child"]
  update.rawInput.agentsStates = {
    "other-child": { status: "completed", message: "CHILD_DONE" },
  }
  rejects(records, "child_mismatch")
})

test("missing, duplicate, unstarted and post-terminal outcomes cannot be called a complete capture", () => {
  rejects(copy().slice(0, -1), "missing_terminal")
  rejects([...copy(), copy().at(-1)], "duplicate_terminal")
  rejects(copy().slice(1), "unstarted_call")
  const duplicate = copy()
  duplicate.splice(1, 0, structuredClone(duplicate[0]))
  rejects(duplicate, "duplicate_call")
  const after = copy()
  after.push(copy()[0])
  rejects(after, "activity_after_terminal")
  const unknown = copy()
  unknown[1].frame.params.update.rawInput.agentsStates["identity-3"].status = "invented"
  rejects(unknown, "unknown_agent_state")
})

test("selection omits arbitrary auth, text, prompt, response and unknown metadata", () => {
  const records = copy()
  const update = records[0].frame.params.update
  update.rawInput.prompt = "/Users/private SECRET_PROMPT"
  update.rawInput.extra = "SECRET_INPUT"
  update._meta.extra = "SECRET_META"
  records[1].frame.params.update.rawInput.agentsStates["identity-3"].message =
    "SECRET_CHILD_RESPONSE"
  records.push({
    direction: "from-agent",
    frame: { method: "_auth/status_update", params: { email: "SECRET_ACCOUNT" } },
  })
  records.push({
    direction: "from-agent",
    frame: {
      method: "session/update",
      params: {
        update: {
          sessionUpdate: "agent_message_chunk",
          content: { text: "SECRET_TEXT" },
        },
      },
    },
  })
  const unknownTool = structuredClone(records[0])
  unknownTool.frame.params.update._meta.codex.collaboration.tool = "SECRET_UNKNOWN_TOOL"
  unknownTool.frame.params.update.title = "SECRET_UNKNOWN_TOOL"
  records.push(unknownTool)
  const selected = selectFrames(records)
  const text = JSON.stringify(selected)
  assert.doesNotMatch(text, /SECRET_|\/Users\//)
  assert.equal(selected.length, fixture.frames.length)
  assert.deepEqual(inspectFrames(selected), inspectFrames(fixture.frames))
})

test("absent delegation, unfinished activity and inconsistent status/title are explicit failures", () => {
  rejects([copy().at(-1)], "missing_native_sequence")
  const unfinished = copy()
  unfinished.splice(5, 1)
  rejects(unfinished, "unfinished_calls")
  const status = copy()
  status[1].frame.params.update.rawInput.status = "failed"
  rejects(status, "status_mismatch")
  const title = copy()
  title[1].frame.params.update.title = "wait"
  rejects(title, "tool_mismatch")
})

test("capture provenance names the checked-in probe and pinned harness lock", () => {
  const hash = (relative) =>
    createHash("sha256")
      .update(readFileSync(new URL(relative, import.meta.url)))
      .digest("hex")
  assert.equal(fixture.source.probeSha256, hash("./capture.mjs"))
  assert.equal(
    fixture.source.lockSha256,
    hash("../../crates/nessa-sdk/harnesses/codex-acp/package-lock.json"),
  )
  assert.match(fixture.source.adapterSha256, /^[0-9a-f]{64}$/)
})

test("selection refuses unexpected retained wire syntax instead of leaking it", () => {
  for (const field of ["status", "title", "sessionUpdate", "kind"]) {
    const records = copy()
    records[0].frame.params.update[field] = "SECRET_UNKNOWN"
    assert.throws(
      () => selectFrames(records),
      (error) => error.code === "invalid_update",
    )
  }
  const status = copy()
  status[0].frame.params.update.rawInput.status = "SECRET_STATUS"
  assert.throws(
    () => selectFrames(status),
    (error) => error.code === "invalid_status",
  )
  const terminal = copy()
  terminal.at(-1).frame.result.stopReason = "SECRET_TERMINAL"
  assert.throws(
    () => selectFrames(terminal),
    (error) => error.code === "invalid_terminal",
  )
})
