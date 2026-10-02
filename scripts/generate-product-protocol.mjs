/** Generate the separate product profile from its JSON Schema. --check detects drift. */
import { readFileSync, writeFileSync, mkdirSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { format, resolveConfig } from "prettier"
import { spawnSync } from "node:child_process"
import {
  coreWireContract,
  applyCoreWireBounds,
} from "./product-protocol/core-contract.mjs"
import { rustWireShapes } from "./product-protocol/rust-wire-shapes.mjs"
import { validateExternalRustTypes } from "./product-protocol/rust-types.mjs"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const schema = JSON.parse(readFileSync(resolve(root, "protocol/product/v1.json"), "utf8"))
const manifest = JSON.parse(
  readFileSync(resolve(root, "protocol/product/manifest.json"), "utf8"),
)
if (
  typeof manifest.handshakeMethod !== "string" ||
  !Object.hasOwn(manifest.methods, manifest.handshakeMethod)
)
  throw new Error("Product handshake method must name an owned manifest method")
const readyMethods = Object.keys(manifest.methods).filter(
  (method) => method !== manifest.handshakeMethod,
)
const ownedSchema = JSON.stringify(schema)
applyCoreWireBounds(schema, coreWireContract(root))
schema.$defs.ProductSessionReady.properties.methods.maxItems = readyMethods.length
let schemaOutput
if (JSON.stringify(schema) !== ownedSchema) {
  if (process.argv.includes("--check"))
    throw new Error("Product schema published bounds are stale")
  schemaOutput = await format(JSON.stringify(schema), {
    ...(await resolveConfig(resolve(root, "prettier.config.js"))),
    parser: "json",
  })
}

// Timing is product policy, not a new wire field. The client must retain the
// correlation through both server phases; a timer above the runtime range wraps.
const timing = schema["x-passiveReadTiming"]
const passiveReadTiming = {}
for (const name of ["readTimeoutMs", "deliveryTimeoutMs", "clientAllowanceMs"]) {
  if (
    !timing ||
    !Object.hasOwn(timing, name) ||
    !Number.isSafeInteger(timing[name]) ||
    timing[name] <= 0
  )
    throw new Error(`Invalid passive read timing: ${name}`)
  passiveReadTiming[name] = timing[name]
}
passiveReadTiming.minRequestTimeoutMs =
  passiveReadTiming.readTimeoutMs +
  passiveReadTiming.deliveryTimeoutMs +
  passiveReadTiming.clientAllowanceMs
if (passiveReadTiming.minRequestTimeoutMs > 2_147_483_647)
  throw new Error("Passive request deadline exceeds the runtime timer range")

const sdkFrames = readFileSync(
  resolve(root, "crates/nessa-sdk/src/infrastructure/session_storage/stream_fact.rs"),
  "utf8",
)
const sdkSource = readFileSync(
  resolve(root, "crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs"),
  "utf8",
)
const ordinaryWire = readFileSync(
  resolve(root, "crates/nessa-server/src/protocol/encode.rs"),
  "utf8",
)
const maxOrdinaryResponseBytes = Number(
  ordinaryWire.match(/MAX_PAYLOAD_BYTES: i64 = ([0-9_]+);/)?.[1].replaceAll("_", ""),
)
const pieceKiB = Number(sdkFrames.match(/MAX_PIECE_BYTES: usize = (\d+) \* 1024;/)?.[1])
const pieceHeader = Number(sdkFrames.match(/PIECE_HEADER_BYTES: usize = (\d+);/)?.[1])
if (
  !pieceKiB ||
  !pieceHeader ||
  !maxOrdinaryResponseBytes ||
  !sdkSource.includes(
    "1 + stream_fact::PIECE_HEADER_BYTES + stream_fact::MAX_PIECE_BYTES;",
  )
)
  throw new Error("SDK physical record maximum needs an explicit generator update")
const maxPhysicalRecordPayloadBytes = 1 + pieceHeader + pieceKiB * 1024
const pageRequest = schema.$defs.RecordPageRequest.properties
for (const field of ["maxPayloadBytes", "maxRecordBytes"]) {
  if (pageRequest[field].maximum !== maxPhysicalRecordPayloadBytes)
    throw new Error(`${field} drifted from the SDK physical maximum`)
}
const snake = (name) => name.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`)
validateExternalRustTypes(schema.$defs)
const externalRustTypes = new Set()
const sharedRustReferences = new Set()
const rustTypeName = (value) => value.split("::").at(-1)
function type(node, rust) {
  if (node.$ref) {
    const name = node.$ref.split("/").at(-1)
    const externalType = schema.$defs[name]["x-rust-type"]
    if (rust && !externalType && sharedOutcomes.has(name)) sharedRustReferences.add(name)
    if (rust && externalType) externalRustTypes.add(externalType)
    return rust ? (externalType ? rustTypeName(externalType) : name) : name
  }
  if (node.enum && !rust) return node.enum.map(JSON.stringify).join(" | ")
  if (Array.isArray(node.type) && rust)
    return `Option<${type({ ...node, type: node.type.find((value) => value !== "null") }, true)}>`
  if (Array.isArray(node.type) && !rust)
    return node.type
      .map((value) => (value === "null" ? "null" : type({ ...node, type: value }, false)))
      .join(" | ")
  if (node.const !== undefined && !rust) return JSON.stringify(node.const)
  if (node.type === "string") return rust ? "String" : "string"
  if (node.type === "integer") return rust ? "u64" : "number"
  if (node.type === "boolean") return rust ? "bool" : "boolean"
  if (node.type === "array")
    return rust ? `Vec<${type(node.items, true)}>` : `${type(node.items, false)}[]`
  throw new Error(`Unsupported schema node: ${JSON.stringify(node)}`)
}
// Keep schema documentation in generated declarations so TypeDoc can render it.
function doc(description) {
  return description ? `/** ${description.replaceAll("*/", "* /")} */\n` : ""
}
let ts =
  "/* eslint-disable */\n/* Generated from protocol/product/v1.json and manifest.json. Do not edit. */\n"
let rs =
  "//! Generated from protocol/product/v1.json. Do not edit.\n//! Bounds are validated at the transport boundary; these are payload types only.\n#![allow(dead_code)]\nuse serde::{Deserialize, Serialize};\nuse serde_json::Value;\n"
const sharedOutcomes = new Set([
  "SessionCloseReason",
  "RecordReadErrorCode",
  "CatalogueReadErrorCode",
])
let contractRs =
  "//! Pure product outcome values generated from protocol/product/v1.json. Do not edit.\nuse serde::{Deserialize, Serialize};\n"
rs += "__SHARED_RUST_IMPORTS__"
rs += "__EXTERNAL_RUST_IMPORTS__"
for (const [name, def] of Object.entries(schema.$defs)) {
  ts += doc(def.description)
  if (def.enum) {
    const pascal = (value) =>
      value
        .split("_")
        .map((p) => p[0].toUpperCase() + p.slice(1))
        .join("")
    ts += `export const ${name} = ${JSON.stringify(Object.fromEntries(def.enum.map((v) => [pascal(v), v])))} as const\nexport type ${name} = typeof ${name}[keyof typeof ${name}]\n`
    let enumRs = ""
    enumRs += `#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]\n#[serde(rename_all = "snake_case")]\npub enum ${name} {${def.enum.map(pascal).join(",")}}\n`
    // The wire spelling, so handlers pass the typed value where a code is written.
    enumRs += `impl ${name} { pub fn as_str(self) -> &'static str { match self {${def.enum.map((v) => `Self::${pascal(v)} => ${JSON.stringify(v)}`).join(",")} } } }\n`
    if (def["x-close-policy"]) {
      ts += `export const sessionClosePolicy = ${JSON.stringify(def["x-close-policy"])} as const\n`
      enumRs += `impl ${name} { pub fn web_socket_code(self) -> u16 { match self {${def.enum.map((v) => `Self::${pascal(v)} => ${def["x-close-policy"][v].webSocketCode}`).join(",")} } } pub fn retryable(self) -> bool { match self {${def.enum.map((v) => `Self::${pascal(v)} => ${def["x-close-policy"][v].retryable}`).join(",")} } } pub(crate) fn from_web_socket_code(code: u16) -> Option<Self> { match code {${def.enum.map((v) => `${def["x-close-policy"][v].webSocketCode} => Some(Self::${pascal(v)})`).join(",")}, _ => None } } }\n`
    }
    if (sharedOutcomes.has(name)) contractRs += enumRs
    else rs += enumRs
    continue
  }
  ts += `export interface ${name} {\n`
  const externalRust = Boolean(def["x-rust-type"])
  // No Debug or Clone: some payloads contain one-time credentials.
  if (!externalRust)
    rs += `#[derive(Deserialize, Serialize)]\n#[serde(rename_all = "camelCase", deny_unknown_fields)]\npub struct ${name} {\n`
  for (const [field, node] of Object.entries(def.properties)) {
    const optional = !def.required.includes(field)
    ts += doc(node.description)
    ts += `  ${field}${optional ? "?" : ""}: ${type(node, false)}\n`
    if (!externalRust)
      rs += `${optional ? '#[serde(default, skip_serializing_if = "Option::is_none")]\n' : ""}pub ${snake(field)}: ${optional ? `Option<${type(node, true).replace(/^Option<(.*)>$/, "$1")}>` : type(node, true)},\n`
  }
  ts += "}\n"
  if (!externalRust) rs += "}\n"
}
/**
 * The one value every array of `field` referring to `kind` agrees on, so a
 * bound that drifted between two commands is a generation failure rather than a
 * constant the client quietly copies from whichever array was read first.
 *
 * A message names two kinds of attachment and they are separate arrays, because
 * they are carried in opposite ways: an image's bytes travel with the message
 * and a linked file's never do. Each array therefore has its own bounds, and
 * each is checked across every command that spells one.
 */
function arrayBound(field, kind, keyword) {
  const arrays = Object.entries(schema.$defs).flatMap(([name, def]) =>
    Object.entries(def.properties ?? {})
      .filter(
        ([property, node]) =>
          property === field && node.items?.$ref?.endsWith(`/${kind}`),
      )
      .map(([, node]) => [name, node[keyword]]),
  )
  if (arrays.length === 0) throw new Error(`No ${field} array carries ${keyword}`)
  const [[, bound]] = arrays
  const disagreeing = arrays.filter(([, value]) => value !== bound)
  if (bound === undefined || disagreeing.length > 0)
    throw new Error(`${field} arrays disagree on ${keyword}: ${JSON.stringify(arrays)}`)
  return bound
}
/** One value the schema states in several places, refused unless they agree. */
function agreeing(name, values) {
  const [first] = values
  if (first === undefined || values.some((value) => value !== first))
    throw new Error(`${name} disagree: ${JSON.stringify(values)}`)
  return first
}
const image = schema.$defs.ImageAttachment.properties
const linked = schema.$defs.LinkedFile.properties
// Named for what a reader of the client says, not for the schema's field paths.
const catalogueDecimalFields = [
  schema.$defs.CatalogueEntryKey.properties.creation,
  schema.$defs.CatalogueDescriptor.properties.revision,
  schema.$defs.CataloguePass.properties.completed,
  schema.$defs.CataloguePass.properties.boundary,
  schema.$defs.CataloguePass.properties.generation,
  schema.$defs.ConversationCatalogueHeadResult.properties.head,
]
const bounds = {
  maxOrdinaryResponseBytes,
  maxReadyMethods: schema.$defs.ProductSessionReady.properties.methods.maxItems,
  maxAuthCredentialCharacters:
    schema.$defs.SessionAuthenticateParams.properties.credential.maxLength,
  maxProductClientIdCharacters:
    schema.$defs.ProductClientMetadata.properties.id.maxLength,
  maxPhysicalRecordPayloadBytes,
  maxRecordPageRecords: pageRequest.maxRecords.maximum,
  maxRecordPagePayloadBytes: pageRequest.maxPayloadBytes.maximum,
  maxRecordResponseBytes: schema.$defs.ConversationRecordsPageResult["x-maxEncodedBytes"],
  minAgentInstallRequestIdCharacters:
    schema.$defs.AgentInstallParams.properties.requestId.minLength,
  maxAgentInstallRequestIdBytes:
    schema.$defs.AgentInstallParams.properties.requestId["x-utf8MaxBytes"],
  maxConfiguredAgents: schema.$defs.AgentsListResult.properties.agents.maxItems,
  maxAgentInstallVersionBytes: agreeing("native installation version bytes", [
    schema.$defs.AgentInstallOffer.properties.version["x-utf8MaxBytes"],
    schema.$defs.AgentInstallResult.properties.version["x-utf8MaxBytes"],
  ]),
  maxImageBytes: image.size.maximum,
  imageMimeTypes: image.mimeType.enum,
  maxMessageImages: arrayBound("attachments", "ImageAttachment", "maxItems"),
  maxMessageImageBytes: arrayBound("attachments", "ImageAttachment", "x-maxTotalBytes"),
  maxUploadBytes: schema.$defs.AttachmentBeginParams.properties.size.maximum,
  maxMessageFiles: arrayBound("files", "LinkedFile", "maxItems"),
  maxFilePathBytes: linked.path["x-utf8MaxBytes"],
  filePathPattern: linked.path.pattern,
  conversationIdPattern: agreeing(
    "conversationId pattern",
    Object.values(schema.$defs).flatMap((def) =>
      def.properties?.conversationId ? [def.properties.conversationId.pattern] : [],
    ),
  ),
  maxConversationTitleBytes: agreeing("conversation title bytes", [
    schema.$defs.ConversationSummary.properties.title["x-utf8MaxBytes"],
    schema.$defs.ConversationView.properties.title["x-utf8MaxBytes"],
  ]),
  maxConversationPreviewBytes:
    schema.$defs.ConversationSummary.properties.preview["x-utf8MaxBytes"],
  maxSyncIdBytes: schema.$defs.RecordScope.properties.receiver["x-utf8MaxBytes"],
  decimalU64Pattern: agreeing("decimal u64 pattern", [
    pageRequest.after.pattern,
    pageRequest.target.pattern,
    schema.$defs.RecordWireRecord.properties.position.pattern,
    schema.$defs.ConversationRecordsHeadResult.properties.head.pattern,
    ...catalogueDecimalFields.map((field) => field.pattern),
  ]),
  maxDecimalU64Characters: agreeing("decimal u64 width", [
    pageRequest.after.maxLength,
    pageRequest.target.maxLength,
    schema.$defs.RecordWireRecord.properties.position.maxLength,
    schema.$defs.ConversationRecordsHeadResult.properties.head.maxLength,
    ...catalogueDecimalFields.map((field) => field.maxLength),
  ]),
  positiveEpochPattern: agreeing(
    "positive access epoch pattern",
    Object.values(schema.$defs).flatMap((def) =>
      def.properties?.accessEpoch?.pattern ? [def.properties.accessEpoch.pattern] : [],
    ),
  ),
  maxPositiveEpochCharacters: agreeing(
    "positive access epoch width",
    Object.values(schema.$defs).flatMap((def) =>
      def.properties?.accessEpoch?.pattern ? [def.properties.accessEpoch.maxLength] : [],
    ),
  ),
  recordPayloadPattern: schema.$defs.RecordWireRecord.properties.payload.pattern,
  minRecordPayloadEncodedCharacters:
    schema.$defs.RecordWireRecord.properties.payload.minLength,
  maxRecordPayloadEncodedCharacters:
    schema.$defs.RecordWireRecord.properties.payload.maxLength,
  maxListedConversations:
    schema.$defs.ConversationListResult.properties.conversations.maxItems,
  maxToolStructuredContentBytes:
    schema.$defs.ConversationTool.properties.structuredContent["x-utf8MaxBytes"],
  maxMcpNameBytes: agreeing("MCP server and tool name bytes", [
    schema.$defs.ConversationMcpTool.properties.server["x-utf8MaxBytes"],
    schema.$defs.ConversationMcpTool.properties.tool["x-utf8MaxBytes"],
  ]),
  maxUiResourceUriBytes:
    schema.$defs.ConversationMcpTool.properties.resourceUri["x-utf8MaxBytes"],
}
for (const name of [
  "maxAuthCredentialCharacters",
  "maxProductClientIdCharacters",
  "minAgentInstallRequestIdCharacters",
  "maxAgentInstallRequestIdBytes",
  "maxPhysicalRecordPayloadBytes",
  "maxRecordPageRecords",
  "maxRecordPagePayloadBytes",
  "maxRecordResponseBytes",
]) {
  rs += `/// Published bound from the product schema.\npub const ${snake(name).toUpperCase()}: usize = ${bounds[name]};\n`
}
for (const [name, value] of Object.entries(passiveReadTiming)) {
  rs += `/// Published passive read timing from the product schema, in milliseconds.\npub const PASSIVE_${snake(name).toUpperCase()}: u64 = ${value};\n`
}
ts += `${doc(
  "Passive source and delivery deadlines, plus the client allowance. The minimum request deadline is their sum; clients raise shorter configured timeouts to this floor.",
)}export const passiveReadTiming = ${JSON.stringify(passiveReadTiming)} as const\n`
ts += `${doc(
  "Bounds the product schema puts on attachments and conversations, generated from it so no copy of a number can drift.",
)}export const bounds = ${JSON.stringify(bounds)} as const\n`
for (const [kind, entries] of [
  ["Method", manifest.methods],
  ["Event", manifest.events],
]) {
  const key = (name) =>
    name
      .split(".")
      .map((part) => part[0].toUpperCase() + part.slice(1))
      .join("")
  rs += `pub mod product_${kind.toLowerCase()} { ${Object.keys(entries)
    .map(
      (name) =>
        `pub const ${snake(key(name)).replace(/^_/, "").toUpperCase()}: &str = ${JSON.stringify(name)};`,
    )
    .join("\n")} }\n`
  ts += `export const Product${kind} = ${JSON.stringify(Object.fromEntries(Object.keys(entries).map((name) => [key(name), name])))} as const\n`
}
rs += rustWireShapes(schema.$defs, [
  "SessionChallenge",
  "SessionAuthenticateParams",
  "ProductSessionReady",
])
rs += `pub const PRODUCT_HANDSHAKE_METHOD: &str = ${JSON.stringify(manifest.handshakeMethod)};
pub const PRODUCT_READY_METHODS: &[&str] = &[${readyMethods.map(JSON.stringify).join(",")}];
`
ts += `export const ProductHandshakeMethod = ${JSON.stringify(manifest.handshakeMethod)} as const
export const productReadyMethods = ${JSON.stringify(readyMethods)} as const
`
rs += `pub const PRODUCT_VERSION: u64 = ${manifest.version};\npub const PRODUCT_SESSION_PATH: &str = ${JSON.stringify(manifest.path)};\n`
function wireShape(node) {
  return Object.fromEntries(
    Object.entries(node)
      .filter(([key]) => key !== "description")
      .map(([key, value]) => [
        key,
        typeof value === "object" && value !== null
          ? Array.isArray(value)
            ? value
            : wireShape(value)
          : value,
      ]),
  )
}
const catalogueWireSchemas = Object.fromEntries(
  Object.entries(schema.$defs)
    .filter(
      ([name]) =>
        name === "RecordScope" ||
        name.startsWith("Catalogue") ||
        name.startsWith("ConversationCatalogue"),
    )
    .map(([name, node]) => [name, wireShape(node)]),
)
ts += `export const catalogueWireSchemas = ${JSON.stringify(catalogueWireSchemas)} as const\n`
ts = await format(ts, {
  ...(await resolveConfig(resolve(root, "prettier.config.js"))),
  parser: "typescript",
})
let imports = ""
const rustImports = new Map()
for (const externalType of externalRustTypes) {
  const separator = externalType.lastIndexOf("::")
  const module = externalType.slice(0, separator)
  const name = externalType.slice(separator + 2)
  const names = rustImports.get(module) ?? []
  names.push(name)
  rustImports.set(module, names)
}
for (const [module, names] of rustImports) {
  imports += `use ${module}::{${names.sort().join(", ")}};\n`
}
rs = rs.replace("__EXTERNAL_RUST_IMPORTS__", imports)
rs = rs.replace(
  "__SHARED_RUST_IMPORTS__",
  sharedRustReferences.size
    ? `use crate::product_contract::generated::{${[...sharedRustReferences].sort().join(",")}};\n`
    : "",
)
const formatted = spawnSync("rustfmt", ["--edition", "2021"], {
  input: rs,
  encoding: "utf8",
})
if (formatted.status !== 0) throw new Error(formatted.stderr)
const formattedContract = spawnSync("rustfmt", ["--edition", "2021"], {
  input: contractRs,
  encoding: "utf8",
})
if (formattedContract.status !== 0) throw new Error(formattedContract.stderr)
const outputs = [
  ...(schemaOutput === undefined ? [] : [["protocol/product/v1.json", schemaOutput]]),
  ["packages/nessa-client/src/generated/product.ts", ts],
  ["crates/nessa-server/src/product/generated.rs", formatted.stdout],
  ["crates/nessa-server/src/product_contract/generated.rs", formattedContract.stdout],
]
for (const [path, contents] of outputs) {
  const target = resolve(root, path)
  if (process.argv.includes("--check")) {
    if (readFileSync(target, "utf8") !== contents)
      throw new Error(`Generated product protocol is stale: ${path}`)
  } else {
    mkdirSync(dirname(target), { recursive: true })
    writeFileSync(target, contents)
  }
}
