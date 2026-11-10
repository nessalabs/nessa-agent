import { strict as assert } from "node:assert"
import { mkdtempSync, rmSync, writeFileSync, appendFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { vmBackgroundEvents } from "./stress-vm-background.mjs"

test("VM telemetry waits for a complete append and rejects malformed complete records", () => {
  const root = mkdtempSync(join(tmpdir(), "vm-background-events-"))
  const path = join(root, "events.jsonl")
  try {
    assert.deepEqual(vmBackgroundEvents(root), [])
    writeFileSync(path, '{"kind":"created"}\n{"kind":"back')
    assert.deepEqual(vmBackgroundEvents(root), [{ kind: "created" }])
    appendFileSync(path, 'ground"}\n')
    assert.deepEqual(vmBackgroundEvents(root), [
      { kind: "created" },
      { kind: "background" },
    ])
    appendFileSync(path, "broken\n")
    assert.throws(() => vmBackgroundEvents(root), SyntaxError)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
