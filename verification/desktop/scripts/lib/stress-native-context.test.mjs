import { strict as assert } from "node:assert"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import {
  contextAudit,
  nativeContextEvents,
  nativeContextExists,
  newContextEvidence,
} from "./stress-native-context.mjs"

test("native trace keeps complete observations and markers name exact used IDs", () => {
  const root = mkdtempSync(join(tmpdir(), "stress-native-evidence-"))
  try {
    assert.deepEqual(nativeContextEvents(root), [])
    const opened = { method: "session/new", sessionId: "fresh", outcome: "opened" }
    writeFileSync(join(root, "events.jsonl"), JSON.stringify(opened) + '\n{"method":')
    assert.deepEqual(nativeContextEvents(root), [opened])
    assert.equal(nativeContextExists(root, "fresh"), false)
    writeFileSync(join(root, "fixture-owned-name.used"), "other")
    assert.equal(nativeContextExists(root, "fresh"), false)
    assert.equal(nativeContextExists(root, "other"), true)
    writeFileSync(join(root, "events.jsonl"), "invalid complete line\n")
    assert.throws(() => nativeContextEvents(root), SyntaxError)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("audit publication identity remains fresh when restart generation repeats", () => {
  const root = mkdtempSync(join(tmpdir(), "stress-context-audit-"))
  try {
    const put = (name, writerId, recordId, sessionId, kind, after) =>
      writeFileSync(
        join(root, name),
        JSON.stringify({
          writerId,
          recordId,
          record: { sessionId, kind, after, generation: 0 },
        }),
      )
    put(
      "first.json",
      "writer-a",
      "record-a",
      "focused",
      "attachment_transition",
      "context_published",
    )
    const before = contextAudit(root, "focused")
    put(
      "restart.json",
      "writer-b",
      "record-b",
      "focused",
      "attachment_transition",
      "context_published",
    )
    put(
      "elsewhere.json",
      "writer-b",
      "record-c",
      "other",
      "attachment_transition",
      "context_published",
    )
    put("queued.json", "writer-b", "record-d", "focused", "queue_admitted", "owned")
    const after = contextAudit(root, "focused")
    assert.deepEqual(newContextEvidence(before.published, after.published), [
      JSON.stringify(["writer-b", "record-b"]),
    ])
    assert.equal(newContextEvidence(before.admitted, after.admitted).length, 1)
    put("malformed.json", undefined, "record-e", "focused", "queue_admitted", "owned")
    assert.throws(() => contextAudit(root, "focused"), /durable record identity/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
