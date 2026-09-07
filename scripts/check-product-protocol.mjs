/** Validate product fixtures and generated Rust/TypeScript contract consistency. */
import Ajv2020 from "ajv/dist/2020.js"
import { readFileSync } from "node:fs"
import { spawnSync } from "node:child_process"
import { fileURLToPath } from "node:url"

const root = fileURLToPath(new URL("../", import.meta.url))
const read = (name) =>
  JSON.parse(
    readFileSync(new URL(`../protocol/product/${name}`, import.meta.url), "utf8"),
  )
const schema = read("v1.json")
const manifest = read("manifest.json")
const ajv = new Ajv2020({ allErrors: true, strict: false })
ajv.addSchema(schema)
for (const name of [
  ...Object.values(manifest.methods),
  ...Object.values(manifest.events),
].filter(Boolean)) {
  if (!schema.$defs[name]) throw new Error(`Missing product schema: ${name}`)
}
for (const [name, fixture] of Object.entries(read("fixtures.json"))) {
  const validate = ajv.getSchema(`${schema.$id}#/$defs/${name}`)
  if (!validate?.(fixture))
    throw new Error(`Invalid ${name} fixture: ${JSON.stringify(validate?.errors)}`)
}
const validateAuth = ajv.getSchema(`${schema.$id}#/$defs/SessionAuthenticateParams`)
const auth = read("fixtures.json").SessionAuthenticateParams
for (const invalid of [
  { ...auth, credential: "" },
  { ...auth, client: { id: "fixture", role: "admin" } },
  { ...auth, nonce: "x".repeat(257) },
]) {
  if (validateAuth(invalid))
    throw new Error("Product auth schema accepts invalid fixture")
}
const result = spawnSync(
  process.execPath,
  ["scripts/generate-product-protocol.mjs", "--check"],
  { cwd: root, stdio: "inherit" },
)
if (result.status !== 0) process.exit(result.status ?? 1)
console.log("Product protocol schema, fixtures, and generated types match")
