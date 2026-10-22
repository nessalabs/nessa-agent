import assert from "node:assert/strict"
import { test } from "node:test"
import { sessionRowMenuMatches } from "./session-row-menu.mjs"

test("the current row menu includes Fork with optional sharing in its product order", () => {
  assert.equal(
    sessionRowMenuMatches(["Rename", "Pin to Sidebar", "Fork", "Archive", "Delete…"]),
    true,
  )
  assert.equal(
    sessionRowMenuMatches([
      "Rename",
      "Pin to Sidebar",
      "Fork",
      "Share…",
      "Archive",
      "Delete…",
    ]),
    true,
  )
})

test("missing, duplicate, unexpected and reordered actions cannot pass", () => {
  for (const words of [
    ["Rename", "Pin to Sidebar", "Archive", "Delete…"],
    ["Rename", "Pin to Sidebar", "Fork", "Delete…"],
    ["Rename", "Pin to Sidebar", "Fork", "Share…", "Share…", "Archive", "Delete…"],
    ["Rename", "Pin to Sidebar", "Fork", "Other", "Archive", "Delete…"],
    ["Rename", "Fork", "Pin to Sidebar", "Archive", "Delete…"],
  ])
    assert.equal(sessionRowMenuMatches(words), false, JSON.stringify(words))
})
