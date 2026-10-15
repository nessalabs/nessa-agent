import assert from "node:assert/strict"
import { readdirSync, readFileSync } from "node:fs"
import test from "node:test"

test("only local Nessa windows receive clipboard text writing, without clipboard reading", () => {
  const grants = readdirSync("src-tauri/capabilities")
    .filter((name) => name.endsWith(".json"))
    .map((name) => JSON.parse(readFileSync(`src-tauri/capabilities/${name}`, "utf8")))
    .filter((capability) =>
      capability.permissions.some((permission) =>
        (typeof permission === "string" ? permission : permission.identifier).startsWith(
          "clipboard-manager:",
        ),
      ),
    )
  assert.deepEqual(grants.flatMap((capability) => capability.windows).sort(), [
    "desktop",
    "main",
    "setup",
  ])
  for (const capability of grants) {
    assert.equal(capability.remote, undefined)
    assert.notEqual(capability.local, false)
    assert.deepEqual(
      capability.permissions.filter((permission) =>
        (typeof permission === "string" ? permission : permission.identifier).startsWith(
          "clipboard-manager:",
        ),
      ),
      ["clipboard-manager:allow-write-text"],
    )
  }
})
