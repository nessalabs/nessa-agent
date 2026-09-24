import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

import { nativeSmokeFixture } from "./native-smoke-fixture.mjs"

const agent = "claude"
const modelId = "claude-haiku-4-5-20251001"
const provider = "anthropic"

const sourceCatalog = JSON.parse(
  readFileSync("crates/nessa-sdk/data/models.json", "utf8"),
)

test("the native smoke fixture keeps its selected binding and catalog model aligned", () => {
  const fixture = nativeSmokeFixture({
    sourceCatalog,
    catalogPath: "/fixture/models.json",
    workspace: "/fixture/workspace",
    providerPath: "/fixture/provider.mjs",
    command: "/fixture/node",
    dataRoot: "/fixture/data",
    instance: "native-smoke-fixture",
    port: 17420,
  })
  const sourceModel = sourceCatalog.models.find(
    (model) => model.provider === provider && model.modelId === modelId,
  )

  assert.equal(fixture.agents.selected, agent)
  assert.deepEqual(Object.keys(fixture.agents.runtimes), [agent])
  assert.equal(fixture.agents.runtimes[agent].model, modelId)
  assert.equal(fixture.catalog.models.length, 1)
  assert.deepEqual(fixture.catalog.models[0], sourceModel)
  assert.equal(fixture.catalog.models[0].provider, provider)
  assert.equal(fixture.catalog.models[0].input.image, true)
  assert.ok(fixture.catalog.models[0].imageInput.mediaTypes.includes("image/png"))
  assert.deepEqual(fixture.settings, {
    onboarding: { completed: true },
    service: {
      dataRoot: "/fixture/data",
      instance: "native-smoke-fixture",
      port: 17420,
    },
  })
})

test("the native smoke fixture refuses a catalog without its image-capable model", () => {
  assert.throws(
    () =>
      nativeSmokeFixture({
        sourceCatalog: { verifiedOn: "fixture", models: [] },
        catalogPath: "/fixture/models.json",
        workspace: "/fixture/workspace",
        providerPath: "/fixture/provider.mjs",
        command: "/fixture/node",
        dataRoot: "/fixture/data",
        instance: "native-smoke-fixture",
        port: 17420,
      }),
    /native smoke model must remain in the bundled catalog/,
  )
})
