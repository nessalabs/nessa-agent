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
ajv.addKeyword({
  keyword: "x-utf8MaxBytes",
  type: "string",
  schemaType: "number",
  validate: (limit, value) => Buffer.byteLength(value, "utf8") <= limit,
})
// What one message's images may weigh together. `maxItems` bounds how many
// there are and `ImageAttachment.size` how heavy one is; neither says this.
ajv.addKeyword({
  keyword: "x-maxTotalBytes",
  type: "array",
  schemaType: "number",
  validate: (limit, value) =>
    value.reduce((total, item) => total + (Number(item?.size) || 0), 0) <= limit,
})
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
// Conversation command correlation and bounds are part of the product contract.
const validateSend = ajv.getSchema(`${schema.$id}#/$defs/ConversationSendParams`)
const send = read("fixtures.json").ConversationSendParams
for (const invalid of [
  { ...send, requestId: "" },
  { ...send, requestId: "😀".repeat(65) },
  { ...send, executionId: "" },
  { ...send, text: "x".repeat(8193) },
  { ...send, executionId: "😀".repeat(65) },
  { ...send, text: "😀".repeat(2049) },
  { ...send, principalId: "untrusted-caller" },
]) {
  if (validateSend(invalid))
    throw new Error("Product conversation schema accepts invalid command")
}
const image = send.attachments[0]
const images = (count, size) => Array.from({ length: count }, () => ({ ...image, size }))
for (const invalid of [
  { ...send, attachments: images(11, 1) },
  { ...send, attachments: images(1, 5_242_881) },
  // Each image is within its own bound; together they are over the message's.
  { ...send, attachments: images(3, 4 * 1024 * 1024) },
]) {
  if (validateSend(invalid))
    throw new Error("Product conversation schema accepts oversized attachments")
}
for (const valid of [
  { ...send, executionId: "😀".repeat(64) },
  { ...send, requestId: "😀".repeat(64) },
  { ...send, text: "😀".repeat(2048) },
  { ...send, attachments: images(10, 1024 * 1024) },
]) {
  if (!validateSend(valid))
    throw new Error(
      `Product conversation schema rejects exact UTF-8 bound: ${JSON.stringify(validateSend.errors)}`,
    )
}
const validateReorder = ajv.getSchema(`${schema.$id}#/$defs/ConversationReorderParams`)
const reorder = read("fixtures.json").ConversationReorderParams
for (const invalid of [
  { ...reorder, executionIds: ["same", "same"] },
  { ...reorder, executionIds: [""] },
  { ...reorder, executionIds: Array.from({ length: 65 }, (_, i) => `e-${i}`) },
  { ...reorder, requestId: "" },
]) {
  if (validateReorder(invalid))
    throw new Error("Product reorder schema accepts invalid command")
}
if (!validateReorder({ ...reorder, executionIds: [] }))
  throw new Error("Empty queue order must be valid")
const result = spawnSync(
  process.execPath,
  ["scripts/generate-product-protocol.mjs", "--check"],
  { cwd: root, stdio: "inherit" },
)
if (result.status !== 0) process.exit(result.status ?? 1)
console.log("Product protocol schema, fixtures, and generated types match")
