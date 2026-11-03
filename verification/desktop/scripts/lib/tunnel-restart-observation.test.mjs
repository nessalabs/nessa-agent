import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"

/**
 * The tunnel step's gateway restart owns its connection-refused lines only
 * when this callback returns true. `stress-restart.test.mjs` keeps the
 * observer strict; this locks the caller that used to fall off the end.
 */
test("the tunnel restart returns success only after its own checks", () => {
  const source = readFileSync(
    new URL("../conversation-stress.mjs", import.meta.url),
    "utf8",
  )
  const marker = "the window did not regain a readable session after tunnel restart"
  const at = source.indexOf(marker)
  assert.ok(at > 0, "tunnel restart readable check missing")
  const windowStart = source.lastIndexOf("restartConsole.during", at)
  const windowEnd = source.indexOf("if (failures.length) return", at)
  assert.ok(windowStart > 0 && windowEnd > at)
  const body = source.slice(windowStart, windowEnd)
  assert.match(body, /after a restart the tunnel is at/)
  assert.match(body, /keptTunnel\(directory\)/)
  assert.match(body, /restartConsole\.readable/)
  assert.match(body, /return failures\.length === before/)
  assert.equal(
    source.slice(windowEnd, windowEnd + 400).includes("after a restart the tunnel is at"),
    false,
  )
})
