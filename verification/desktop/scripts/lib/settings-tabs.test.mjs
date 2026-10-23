import assert from "node:assert/strict"
import { test } from "node:test"
import { settingsTabFailures } from "./settings-tabs.mjs"

test("Advanced requires both published tabs in their catalogue order", () => {
  assert.deepEqual(settingsTabFailures("advanced", ["Experimental", "Setup"]), [])
  for (const shown of [[], ["Experimental"], ["Setup", "Experimental"], ["Other"]])
    assert.equal(settingsTabFailures("advanced", shown).length, 1)
})

test("a single-tab category keeps its strip absent", () => {
  assert.deepEqual(settingsTabFailures("about", []), [])
  assert.equal(settingsTabFailures("about", ["About"]).length, 1)
})
