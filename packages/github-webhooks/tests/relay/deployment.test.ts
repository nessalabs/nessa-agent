import { test } from "node:test"
import assert from "node:assert/strict"
import {
  configuredDevelopment,
  deployment,
  stageConfiguration,
} from "../../scripts/deployment.mjs"
test("rollout refuses missing stages and resource reuse before deploying", () => {
  const dev = configuredDevelopment()
  assert.equal(deployment("dev", { dev }).vars.RELAY_STAGE, "dev")
  for (const stage of ["prod", "alpha", "ci", "unknown"])
    assert.throws(() => deployment(stage, { dev, prod: null }))
  assert.throws(() =>
    deployment("dev", { dev, prod: { ...dev, vars: { RELAY_STAGE: "prod" } } }),
  )
})

test("production cannot reuse the development GitHub App or relay", () => {
  const dev = {
    origin: "https://dev.example",
    appSlug: "dev-app",
    clientId: "dev-client",
  }
  assert.throws(() => stageConfiguration("prod", { dev, prod: null }))
  assert.throws(() =>
    stageConfiguration("prod", { dev, prod: { ...dev, origin: "https://prod.example" } }),
  )
  assert.equal(
    stageConfiguration("prod", {
      dev,
      prod: {
        origin: "https://prod.example",
        appSlug: "prod-app",
        clientId: "prod-client",
      },
    }).appSlug,
    "prod-app",
  )
})
