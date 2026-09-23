import assert from "node:assert/strict"

const nativeSmokeAgent = "claude"
const nativeSmokeModel = "claude-haiku-4-5-20251001"
const nativeSmokeProvider = "anthropic"

/**
 * Select the production catalog record and keep the fixture's runtime identity
 * aligned with the provider binding that can actually accept image bytes.
 */
export function nativeSmokeFixture({
  sourceCatalog,
  catalogPath,
  workspace,
  providerPath,
  command,
}) {
  const model = sourceCatalog.models.find(
    (candidate) =>
      candidate.provider === nativeSmokeProvider &&
      candidate.modelId === nativeSmokeModel,
  )
  assert.ok(model, "native smoke model must remain in the bundled catalog")
  assert.equal(model.input?.image, true, "native smoke model must accept images")
  assert.ok(
    model.imageInput?.mediaTypes?.includes("image/png"),
    "native smoke model must accept PNG images",
  )

  return {
    catalog: {
      verifiedOn: sourceCatalog.verifiedOn,
      models: [model],
    },
    agents: {
      catalog: catalogPath,
      workspace,
      selected: nativeSmokeAgent,
      runtimes: {
        [nativeSmokeAgent]: {
          command,
          args: [providerPath],
          model: model.modelId,
          toolsEnabled: true,
        },
      },
    },
  }
}
