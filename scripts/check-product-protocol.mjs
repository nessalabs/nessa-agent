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
const validateLifecycle = ajv.getSchema(`${schema.$id}#/$defs/ConversationLifecycle`)
for (const invalid of [
  { phase: "failed" },
  { phase: "starting", failure: { code: "provider", message: "failed" } },
  { phase: "failed", failure: { code: "provider", message: "😀".repeat(513) } },
  {
    phase: "attached",
    evidenceFailure: { code: "audit", message: "😀".repeat(513) },
  },
  {
    phase: "attached",
    evidenceFailure: { code: "provider", message: "failed" },
  },
]) {
  if (validateLifecycle(invalid))
    throw new Error("Product conversation lifecycle accepts contradictory fields")
}
if (
  !validateLifecycle({
    phase: "failed",
    failure: { code: "provider", message: "😀".repeat(512) },
  })
)
  throw new Error("Product conversation lifecycle rejects an exact UTF-8 bound")
if (
  !validateLifecycle({
    phase: "attached",
    evidenceFailure: { code: "audit", message: "😀".repeat(512) },
  })
)
  throw new Error("Product lifecycle evidence rejects an exact UTF-8 bound")
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
// The list's bounds: what a row may say, how many rows there are, and that
// what a summary has not got is null rather than an empty string.
const validateList = ajv.getSchema(`${schema.$id}#/$defs/ConversationListResult`)
const listed = read("fixtures.json").ConversationListResult
const row = listed.conversations[0]
for (const invalid of [
  { complete: true, conversations: [{ ...row, title: "" }] },
  { complete: true, conversations: [{ ...row, preview: "" }] },
  { complete: true, conversations: [{ ...row, title: "😀".repeat(65) }] },
  { complete: true, conversations: [{ ...row, preview: "😀".repeat(129) }] },
  { complete: true, conversations: [{ ...row, archived: undefined }] },
  { complete: true, conversations: [{ ...row, unread: 1 }] },
  { complete: true, conversations: Array.from({ length: 501 }, () => row) },
  // Whether the bound left any out is always said.
  { conversations: [row] },
  { complete: "yes", conversations: [row] },
]) {
  if (validateList(JSON.parse(JSON.stringify(invalid))))
    throw new Error("Product conversation list accepts an out-of-bounds row")
}
if (
  !validateList({
    conversations: [{ ...row, title: "😀".repeat(64), preview: "😀".repeat(128) }],
    complete: false,
  })
)
  throw new Error("Product conversation list rejects an exact UTF-8 bound")
// The view's title follows the list's bounds: null before anything was said,
// never an empty string, never over the byte bound.
const validateView = ajv.getSchema(`${schema.$id}#/$defs/ConversationView`)
const view = read("fixtures.json").ConversationView
for (const invalid of [
  { ...view, title: "" },
  { ...view, title: "😀".repeat(65) },
  { ...view, title: undefined },
]) {
  if (validateView(JSON.parse(JSON.stringify(invalid))))
    throw new Error("Product conversation view accepts an invalid title")
}
if (!validateView({ ...view, title: null }))
  throw new Error("Product conversation view rejects a null title")
const validateListParams = ajv.getSchema(`${schema.$id}#/$defs/ConversationListParams`)
for (const invalid of [{ archived: "yes" }, { archived: true, cursor: "x" }]) {
  if (validateListParams(invalid))
    throw new Error("Product conversation list accepts invalid params")
}
for (const [name, fixture] of [
  ["ConversationArchiveParams", read("fixtures.json").ConversationArchiveParams],
  ["ConversationDeleteParams", read("fixtures.json").ConversationDeleteParams],
]) {
  const validate = ajv.getSchema(`${schema.$id}#/$defs/${name}`)
  for (const invalid of [
    { ...fixture, requestId: "" },
    { ...fixture, requestId: "😀".repeat(65) },
    { ...fixture, conversationId: "not-a-uuid" },
    { ...fixture, reason: "because" },
  ]) {
    if (validate(invalid)) throw new Error(`Product ${name} accepts an invalid command`)
  }
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
// A linked file names a path and carries no bytes, so what the schema can
// refuse is the shape of the path. The gateway's domain refuses the rest.
const files = (paths) => paths.map((path) => ({ path }))
for (const invalid of [
  { ...send, files: files(["report.pdf"]) },
  { ...send, files: files(["./report.pdf"]) },
  { ...send, files: files([""]) },
  { ...send, files: files(["/tmp/a\nb.pdf"]) },
  { ...send, files: files(["/tmp/a\u0000b.pdf"]) },
  // C1 controls and DEL, which the gateway refuses with `char::is_control`.
  { ...send, files: files(["/tmp/a\u0085b.pdf"]) },
  { ...send, files: files(["/tmp/a\u009fb.pdf"]) },
  { ...send, files: files(["/tmp/a\u007fb.pdf"]) },
  // Components that do not survive being written as a URI.
  { ...send, files: files(["/tmp/"]) },
  { ...send, files: files(["/"]) },
  { ...send, files: files(["//tmp/a.pdf"]) },
  { ...send, files: files(["/tmp//a.pdf"]) },
  { ...send, files: files(["/tmp/."]) },
  { ...send, files: files(["/tmp/.."]) },
  { ...send, files: files(["/tmp/../etc/passwd"]) },
  { ...send, files: files(["/tmp/./a.pdf"]) },
  { ...send, files: files([`/${"a".repeat(4096)}`]) },
  // Over the bound in bytes while under it in code points, which is the only
  // case that tells the two apart: `maxLength` counts code points and takes
  // this, so `x-utf8MaxBytes` is the rule doing the work, and it has to be the
  // same rule the gateway applies.
  { ...send, files: files([`/${"\u0451".repeat(2048)}`]) },
  { ...send, files: files(Array.from({ length: 11 }, (_, i) => `/tmp/${i}.pdf`)) },
  { ...send, files: [{ path: "/tmp/a.pdf", name: "a.pdf" }] },
]) {
  if (validateSend(invalid))
    throw new Error(
      `Product conversation schema accepts an unusable file path: ${JSON.stringify(invalid.files)}`,
    )
}
for (const valid of [
  { ...send, executionId: "😀".repeat(64) },
  { ...send, requestId: "😀".repeat(64) },
  { ...send, text: "😀".repeat(2048) },
  { ...send, attachments: images(10, 1024 * 1024) },
  { ...send, files: [] },
  // Spaces, parentheses, quotes, and non-Latin names are ordinary file names.
  { ...send, files: files([`/tmp/it's a "report" (final) 100%.pdf`, "/tmp/отчёт.pdf"]) },
  // So are brackets and backslashes. The gateway hands the path to the agent
  // inside a markdown link and encodes both halves of that link down to an
  // allowlist, so nothing downstream depends on a path not holding one.
  {
    ...send,
    files: files(["/tmp/[draft] notes.pdf", "/tmp/a]b.pdf", "/tmp/back\\slash"]),
  },
  { ...send, files: files([`/${"a".repeat(4095)}`]) },
  // Exactly 4096 bytes and only 2049 code points: the bound is bytes.
  { ...send, files: files([`/${"\u0451".repeat(2047)}a`]) },
  // A leading dot is a hidden file and a name of three dots is a name.
  { ...send, files: files(["/Users/ada/.zshrc", "/Users/ada/...", "/tmp/a:b.pdf"]) },
  { ...send, files: files(Array.from({ length: 10 }, (_, i) => `/tmp/${i}.pdf`)) },
]) {
  if (!validateSend(valid))
    throw new Error(
      `Product conversation schema rejects exact UTF-8 bound: ${JSON.stringify(validateSend.errors)}`,
    )
}
// The agent a creation names is bounded like every other bounded string here:
// in UTF-8 bytes, not code units. Without that, a name of nine emoji is 18 code
// units and passes `maxLength`, while the gateway's own bound counts 36 bytes.
const validateCreate = ajv.getSchema(`${schema.$id}#/$defs/ConversationCreateParams`)
const create = read("fixtures.json").ConversationCreateParams
for (const invalid of [
  { ...create, agent: "" },
  { ...create, agent: "x".repeat(33) },
  { ...create, agent: "\u{1f600}".repeat(9) },
]) {
  if (validateCreate(invalid))
    throw new Error("Product conversation schema accepts invalid agent")
}
if (!validateCreate({ ...create, agent: "\u{1f600}".repeat(8) }))
  throw new Error(
    `Product conversation schema rejects exact UTF-8 bound: ${JSON.stringify(validateCreate.errors)}`,
  )
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
