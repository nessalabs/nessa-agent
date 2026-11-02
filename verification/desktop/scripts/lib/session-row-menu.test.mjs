import assert from "node:assert/strict"
import { test } from "node:test"
import { sessionRowMenuMatches } from "./session-row-menu.mjs"

test("the current row menu includes Fork with optional sharing in its product order", () => {
  assert.equal(
    sessionRowMenuMatches(["Rename", "Pin", "Fork", "Archive", "Delete…"]),
    true,
  )
  assert.equal(
    sessionRowMenuMatches(["Rename", "Pin", "Fork", "Share…", "Archive", "Delete…"]),
    true,
  )
})

test("missing, duplicate, unexpected and reordered actions cannot pass", () => {
  for (const words of [
    ["Rename", "Pin", "Archive", "Delete…"],
    ["Rename", "Pin", "Fork", "Delete…"],
    ["Rename", "Pin", "Fork", "Share…", "Share…", "Archive", "Delete…"],
    ["Rename", "Pin", "Fork", "Other", "Archive", "Delete…"],
    ["Rename", "Fork", "Pin", "Archive", "Delete…"],
  ])
    assert.equal(sessionRowMenuMatches(words), false, JSON.stringify(words))
})

test("a pinned row offers Unpin in the same position", () => {
  assert.equal(
    sessionRowMenuMatches(["Rename", "Unpin", "Fork", "Archive", "Delete…"]),
    true,
  )
  assert.equal(
    sessionRowMenuMatches(["Rename", "Pin to Sidebar", "Fork", "Archive", "Delete…"]),
    false,
  )
})
