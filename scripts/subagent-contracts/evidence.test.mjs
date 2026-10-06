import { createHash } from "node:crypto"
import { strict as assert } from "node:assert"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { EvidenceError, inspectCapture, selectCapture } from "./evidence.mjs"

const fixture = JSON.parse(
  readFileSync(new URL("./fixtures/codex-native.json", import.meta.url), "utf8"),
)
const copy = () => structuredClone(fixture.frames)
const inspect = (records, admission = fixture.admission) =>
  inspectCapture({ admission, frames: records })
const select = (records) =>
  selectCapture(
    Object.freeze({
      kind: "sealed-acp-probe",
      admissionJson: JSON.stringify(fixture.admission),
      recordingJson: JSON.stringify(records),
    }),
  ).frames
const rejects = (records, code) =>
  assert.throws(
    () => inspect(records),
    (error) => error instanceof EvidenceError && error.code === code,
  )

test("live native spawn, wait and close retain one parent and one child across independent wire reports", () => {
  assert.deepEqual(inspect(fixture.frames), {
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
  rejects(records, "call_target_mismatch")
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
  const selected = select(records)
  const text = JSON.stringify(selected)
  assert.doesNotMatch(text, /SECRET_|\/Users\//)
  assert.equal(selected.length, fixture.frames.length)
  assert.deepEqual(inspect(selected), inspect(fixture.frames))
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
  assert.equal(fixture.source.sessionSha256, hash("./acp-session.mjs"))
  assert.equal(fixture.source.processSha256, hash("./processes.mjs"))
  assert.equal(fixture.source.selectorSha256, hash("./evidence.mjs"))
  assert.equal(fixture.source.metadataSha256, hash("./metadata.mjs"))
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
      () => select(records),
      (error) => error.code === "invalid_update",
    )
  }
  const status = copy()
  status[0].frame.params.update.rawInput.status = "SECRET_STATUS"
  assert.throws(
    () => select(status),
    (error) => error.code === "invalid_status",
  )
  const terminal = copy()
  terminal.at(-1).frame.result.stopReason = "SECRET_TERMINAL"
  assert.throws(
    () => select(terminal),
    (error) => error.code === "invalid_terminal",
  )
})

test("wait and close retain their admitted child before a matching completion can settle them", () => {
  for (const index of [2, 4]) {
    const records = copy()
    records[index].frame.params.update.rawInput.receiverThreadIds = ["foreign-child"]
    records[index].frame.params.update._meta.codex.collaboration.receiverThreadIds = [
      "foreign-child",
    ]
    rejects(records, "child_mismatch")
  }
  // A coherent start can still be contradicted by a coherent completion.
  const completion = copy()
  const update = completion[3].frame.params.update
  update.rawInput.receiverThreadIds = ["foreign-child"]
  update._meta.codex.collaboration.receiverThreadIds = ["foreign-child"]
  update.rawInput.agentsStates = {
    "foreign-child": { status: "completed", message: "CHILD_DONE" },
  }
  rejects(completion, "call_target_mismatch")
  // Spawn can name its child up front, provided completion names that child.
  const upfront = copy()
  upfront[0].frame.params.update.rawInput.receiverThreadIds = ["identity-3"]
  upfront[0].frame.params.update._meta.codex.collaboration.receiverThreadIds = [
    "identity-3",
  ]
  assert.deepEqual(inspect(upfront), inspect(fixture.frames))
})

test("cancelled parent and failed or wrong-result child reports are typed unsuccessful evidence", () => {
  const cancelled = copy()
  cancelled.at(-1).frame.result.stopReason = "cancelled"
  rejects(cancelled, "terminal_correlation_mismatch")
  const cancelledAdmission = structuredClone(fixture.admission)
  cancelledAdmission.prompt.stopReason = "cancelled"
  assert.throws(
    () => inspect(cancelled, cancelledAdmission),
    (error) => error.code === "unexpected_terminal",
  )
  for (const index of [3, 5]) {
    const errored = copy()
    errored[index].frame.params.update.rawInput.agentsStates["identity-3"].status =
      "errored"
    rejects(errored, "child_not_completed")
    const wrong = copy()
    wrong[index].frame.params.update.rawInput.agentsStates["identity-3"].message =
      "<omitted>"
    rejects(wrong, "child_result_mismatch")
  }
})

test("independent opening/prompt/close evidence governs all normalized frames", () => {
  const foreign = copy()
  for (const { frame } of foreign) {
    if (!frame.params) continue
    frame.params.sessionId = "foreign-parent"
    frame.params.update.rawInput.senderThreadId = "foreign-parent"
    frame.params.update._meta.codex.collaboration.senderThreadId = "foreign-parent"
  }
  rejects(foreign, "admitted_session_mismatch")
  const inventedTerminal = copy()
  inventedTerminal.at(-1).frame.id = 999
  rejects(inventedTerminal, "terminal_correlation_mismatch")
  const wrongPrompt = structuredClone(fixture.admission)
  wrongPrompt.prompt.responseId = 999
  assert.throws(
    () => inspect(copy(), wrongPrompt),
    (error) => error.code === "rpc_correlation_mismatch",
  )
  const wrongClose = structuredClone(fixture.admission)
  wrongClose.close.acknowledged = false
  assert.throws(
    () => inspect(copy(), wrongClose),
    (error) => error.code === "close_unconfirmed",
  )
})
