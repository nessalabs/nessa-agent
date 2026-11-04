/** A manifest method is a CLI and MCP entry, or an exclusion with a reason. */
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { buildSurfaceCatalog } from "./surface-catalog.mjs"

const root = fileURLToPath(new URL("../../", import.meta.url))
const read = (name) =>
  JSON.parse(readFileSync(new URL(`../../${name}`, import.meta.url), "utf8"))

function sources() {
  return {
    manifest: read("protocol/product/manifest.json"),
    schema: read("protocol/product/v1.json"),
    exclusions: read("protocol/product/surface-exclusions.json"),
    hints: read("protocol/product/surface-hints.json"),
  }
}

test("every manifest method is exposed or excluded with a reason", () => {
  const catalog = buildSurfaceCatalog(sources())
  const manifest = sources().manifest
  const exposed = new Set(catalog.methods.map((method) => method.name))
  const excluded = new Map(catalog.excluded.map((entry) => [entry.method, entry.reason]))
  for (const method of Object.keys(manifest.methods)) {
    const listed = exposed.has(method)
    const reason = excluded.get(method)
    assert.equal(listed || typeof reason === "string", true, method)
    assert.equal(listed && reason !== undefined, false, method)
    if (reason !== undefined) assert.notEqual(reason.trim(), "")
  }
  assert.equal(
    exposed.size + excluded.size,
    Object.keys(manifest.methods).length,
  )
})

test("a new manifest method with no entry and no exclusion fails", () => {
  const input = sources()
  input.manifest = structuredClone(input.manifest)
  input.manifest.methods["example.extra"] = { params: null, grant: "server.read" }
  assert.throws(
    () => buildSurfaceCatalog(input),
    /example\.extra has no CLI or MCP entry and is not excluded/,
  )
})

test("an exclusion without a reason fails", () => {
  const input = sources()
  input.exclusions = { ...input.exclusions, "server.health": "   " }
  input.hints = { ...input.hints }
  delete input.hints["server.health"]
  assert.throws(
    () => buildSurfaceCatalog(input),
    /server\.health needs a written reason/,
  )
})

test("excluding a method removes it from the exposed list", () => {
  const input = sources()
  input.exclusions = {
    ...input.exclusions,
    "server.health": "Kept off this surface for the test.",
  }
  input.hints = { ...input.hints }
  delete input.hints["server.health"]
  const catalog = buildSurfaceCatalog(input)
  assert.equal(
    catalog.methods.some((method) => method.name === "server.health"),
    false,
  )
  assert.equal(
    catalog.excluded.find((entry) => entry.method === "server.health")?.reason,
    "Kept off this surface for the test.",
  )
})
