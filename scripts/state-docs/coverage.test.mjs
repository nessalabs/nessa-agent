import { test } from "node:test"
import assert from "node:assert/strict"
import { modelCoverage } from "./coverage.mjs"

function page(id, file, body = "", extra = {}) {
  return {
    summary: {
      id,
      file,
      title: id,
      kind: "flow",
      status: "implemented",
      parent: "root",
      ...extra,
    },
    body,
  }
}
function catalog(...pages) {
  return new Map(pages.map((entry) => [entry.summary.id, entry]))
}

test("links elsewhere do not disguise an unmapped feature as formal coverage", () => {
  const owner = page("owner", "docs/state/services/owner.md", "", {
    model: { spec: "tla/Owner.tla" },
  })
  const unrelated = page("flow", "docs/state/services/flow.md", "See [owner](owner.md).")
  const report = modelCoverage(catalog(owner, unrelated))
  assert.equal(report.ok, false)
  assert.equal(report.entries[1].disposition, "unmodeled")
})

test("shared coverage resolves an explicit owning page and does not count as a formal pass", () => {
  const owner = page("owner", "docs/state/services/owner.md", "", {
    model: { spec: "tla/Owner.tla" },
  })
  const shared = page(
    "flow",
    "docs/state/services/ui/flow.md",
    "## Model coverage\n\nThe [owner](../owner.md#scope) models command admission only.\n\n## Other\nIgnored.",
  )
  const result = modelCoverage(catalog(owner, shared))
  assert.equal(result.ok, true)
  assert.deepEqual(result.entries[1].owners, ["owner"])
  assert.equal(result.entries[1].evidence, "see-owner-scope")
  assert.equal(result.entries[0].evidence, "not-run")
})

test("indexes, contracts and external boundaries remain distinct from checked models", () => {
  const root = page("root", "docs/state/README.md", "", { parent: null, kind: "system" })
  const contract = page("value", "docs/state/value.md", "", { kind: "contract" })
  const external = page(
    "external",
    "docs/state/external.md",
    "## Model coverage\n\nDisposition: external-source boundary. The sibling renderer is outside this repository.",
  )
  const stale = page("old", "docs/state/old.md", "", {
    model: {},
    modelCheckStale: true,
    modelCheck: { held: ["Safe"], broke: [], unchecked: [] },
  })
  const result = modelCoverage(catalog(root, contract, external, stale))
  assert.deepEqual(
    result.entries.map((entry) => entry.disposition),
    ["index", "contract", "external", "direct"],
  )
  assert.equal(result.entries[3].evidence, "stale")
})

test("an exclusion mentioning external providers does not conceal an unmodeled local owner", () => {
  const local = page(
    "local",
    "docs/state/local.md",
    "## Model coverage\n\nLocal focus transitions remain unmodeled; external provider behavior is excluded.",
  )
  assert.equal(modelCoverage(catalog(local)).entries[0].disposition, "unmodeled")
})

test("having child pages cannot disguise an unmodeled lifecycle owner as an index", () => {
  const owner = page("owner", "docs/state/owner.md", "", { kind: "statechart" })
  const child = page("value", "docs/state/value.md", "", {
    kind: "contract",
    parent: "owner",
  })
  const result = modelCoverage(catalog(owner, child))
  assert.equal(result.ok, false)
  assert.equal(result.entries[0].disposition, "unmodeled")
})
