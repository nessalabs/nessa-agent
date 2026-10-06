import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { it } from "node:test"

import { seededLoadSpec, seededSearch } from "./seeded-load.mjs"

const dryRun =
  "?seeded=590&now=1700000000000&sessions=10000&longTranscripts=1&messages=8&messageCharacters=4000"

it("the workspace-load query is the dry run", () => {
  assert.equal(seededSearch(seededLoadSpec), dryRun)
  assert.equal(seededSearch(seededLoadSpec).includes("gateway"), false)
})

it("the page parser accepts that query", async () => {
  const { register } = await import("tsx/esm/api")
  register()
  const { seededWorkspaceSpec } = await import(
    new URL(
      "../../../../src/desktop/workspace/adapters/in-memory/seeded-workspace.ts",
      import.meta.url,
    ).href
  )
  assert.deepEqual(seededWorkspaceSpec(dryRun), {
    seed: 590,
    now: 1_700_000_000_000,
    sessions: 10_000,
    longTranscripts: 1,
    messages: 8,
    messageCharacters: 4_000,
  })
})

it("run-all does not run workspace-load", () => {
  const source = readFileSync(new URL("../run-all.mjs", import.meta.url), "utf8")
  assert.equal(source.includes('"workspace-load"'), false)
})
