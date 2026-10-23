import test from "node:test"
import assert from "node:assert/strict"
import { adequatelySandboxed } from "./sandbox-status.mjs"

test("current Chromium namespace and SUID rows require seccomp and positive evaluation", () => {
  const good = "You are adequately sandboxed."
  const rows = [
    ["Layer 1 Sandbox", "Namespace"],
    ["PID namespaces", "Yes"],
    ["Seccomp-BPF sandbox", "Yes"],
  ]
  assert.equal(adequatelySandboxed(rows, good), true)
  assert.equal(
    adequatelySandboxed([["Layer 1 Sandbox", "SUID"], ...rows.slice(1)], good),
    true,
  )
  for (let index = 0; index < rows.length; index++) {
    const bad = rows.map((row, i) => (i === index ? [row[0], "No"] : row))
    assert.equal(adequatelySandboxed(bad, good), false)
  }
  assert.equal(adequatelySandboxed(rows, "You are NOT adequately sandboxed."), false)
  assert.equal(adequatelySandboxed([], good), false)
})
