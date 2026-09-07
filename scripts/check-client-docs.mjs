/** Verify that source documentation survives schema generation and TypeDoc export. */
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"

const read = (path) =>
  JSON.parse(readFileSync(new URL(`../${path}`, import.meta.url), "utf8"))
const api = read("docs/generated/client-api.json")
const normalize = (text) => text.replace(/\s+/g, " ").trim()
const summary = (node) =>
  normalize((node?.comment?.summary ?? []).map((part) => part.text).join(""))
const exported = (name) =>
  api.children.find((node) => node.name === name && node.kind === 256)

for (const path of [
  "protocol/product/v1.json",
  ...["common", "frames", "server", "conversation", "shortcuts"].map(
    (name) => `protocol/schemas/v1/${name}.json`,
  ),
]) {
  for (const [name, definition] of Object.entries(read(path).$defs)) {
    const reflection = exported(name)
    if (!reflection) continue // Only public interfaces are part of this reference.
    assert(definition.description, `${name} needs a source description`)
    assert.equal(
      summary(reflection),
      normalize(definition.description),
      `${name} lost its source description`,
    )
    for (const [field, property] of Object.entries(definition.properties ?? {})) {
      assert(property.description, `${name}.${field} needs a source description`)
      assert.equal(
        summary(reflection.children?.find((node) => node.name === field)),
        normalize(property.description),
        `${name}.${field} lost its source description`,
      )
    }
  }
}
for (const name of ["NessaClient", "NessaClientConfig"]) {
  const reflection = api.children.find((node) => node.name === name)
  assert(summary(reflection), `${name} needs an explanation`)
  for (const member of reflection.children ?? []) {
    if (member.inheritedFrom) continue
    const candidates = [member, ...(member.signatures ?? []), member.getSignature].filter(
      Boolean,
    )
    assert(
      candidates.some((node) => summary(node)),
      `${name}.${member.name} needs an explanation`,
    )
  }
}
console.log("Public protocol descriptions and SDK class documentation survive generation")
