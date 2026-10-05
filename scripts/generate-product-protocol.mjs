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
import {
  derivePairingValues,
  pairingArrayOwner,
  pairingValueSchema,
} from "./product-protocol/pairing-values.mjs"
import { rustWireShapes } from "./product-protocol/rust-wire-shapes.mjs"
import { validateExternalRustTypes } from "./product-protocol/rust-types.mjs"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const pairingDirectory = "crates/nessa-auth/src/domain/pairing/value_objects"
const pairingValues = JSON.parse(
  readFileSync(resolve(root, `${pairingDirectory}/wire-values.json`), "utf8"),
)
const pairing = pairingValueSchema(pairingValues)
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
// Pairing identity, key and code widths have one owner: Auth's wire-values.json.
schema.$defs = derivePairingValues(schema, pairingValues).schema.$defs
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

// How long an MCP App's calls can take the gateway, with one owner: a review
// waiting for the person, the server's budgets for a call and a read, and the
// client's allowance. The client waits their sum for `mcp.callTool` and, since
// the gateway may open the conversation first, for `mcp.readResource` too;
// every layer reads these generated values and spells none of them.
const appTiming = schema["x-mcpAppCallTiming"]
const mcpAppCallTiming = {}
for (const name of [
  "reviewDeadlineMs",
  "callTimeoutMs",
  "readTimeoutMs",
  "clientAllowanceMs",
]) {
  if (
    !appTiming ||
    !Object.hasOwn(appTiming, name) ||
    !Number.isSafeInteger(appTiming[name]) ||
    appTiming[name] <= 0
  )
    throw new Error(`Invalid MCP App call timing: ${name}`)
  mcpAppCallTiming[name] = appTiming[name]
}
if (Object.keys(appTiming).length !== Object.keys(mcpAppCallTiming).length)
  throw new Error("MCP App call timing has unknown fields")
mcpAppCallTiming.callDeadlineMs =
  mcpAppCallTiming.reviewDeadlineMs +
  mcpAppCallTiming.callTimeoutMs +
  mcpAppCallTiming.clientAllowanceMs
if (mcpAppCallTiming.callDeadlineMs > 2_147_483_647)
  throw new Error("MCP App call deadline exceeds the runtime timer range")
// A client waits a call's deadline for a read too; a read longer than a call
// would be abandoned while the gateway is still bound to answer it.
if (mcpAppCallTiming.readTimeoutMs > mcpAppCallTiming.callTimeoutMs)
  throw new Error("MCP App read timeout outlasts a call")

// How long, how far and how many at once mcpServers.inspect runs, with one
// owner: the gateway reads the bounds as generated constants, and the client
// waits the deadline plus its allowance (the server's stop and the audit
// records come after the deadline).
const inspectPolicy = schema["x-mcpServerInspect"]
const mcpServerInspect = {}
for (const name of [
  "deadlineMs",
  "maxToolPages",
  "maxUiReads",
  "maxConcurrent",
  "clientAllowanceMs",
]) {
  if (
    !inspectPolicy ||
    !Object.hasOwn(inspectPolicy, name) ||
    !Number.isSafeInteger(inspectPolicy[name]) ||
    inspectPolicy[name] <= 0
  )
    throw new Error(`Invalid MCP server inspection policy: ${name}`)
  mcpServerInspect[name] = inspectPolicy[name]
}
if (Object.keys(inspectPolicy).length !== Object.keys(mcpServerInspect).length)
  throw new Error("MCP server inspection policy has unknown fields")
mcpServerInspect.requestDeadlineMs =
  mcpServerInspect.deadlineMs + mcpServerInspect.clientAllowanceMs
if (mcpServerInspect.requestDeadlineMs > 2_147_483_647)
  throw new Error("MCP server inspection deadline exceeds the runtime timer range")

// The rules for a stored MCP server, owned by the SDK (StdioMcpServer::problem,
// problem_in and McpServerLaunch::problem) and published as schema data so the
// schema's prose and a client name the same numbers: each value must equal the
// SDK constant it names, or generation fails.
const mcpServerRules = {}
{
  const rules = schema["x-mcpServerRules"]
  const owners = {
    maxServers: ["acp/sessions/config.rs", "MAX_MCP_SERVERS"],
    nameMaxBytes: ["acp/sessions/config.rs", "MAX_MCP_SERVER_NAME_BYTES"],
    maxArgs: ["acp/sessions/config.rs", "MAX_MCP_SERVER_ARGS"],
    argMaxBytes: ["acp/sessions/config.rs", "MAX_MCP_SERVER_ARG_BYTES"],
    environmentNameMaxBytes: ["mcp/servers.rs", "MAX_MCP_ENVIRONMENT_NAME_BYTES"],
  }
  if (!rules || Object.keys(rules).length !== Object.keys(owners).length)
    throw new Error("MCP server rules must name exactly the SDK's bounds")
  for (const [name, [file, constant]] of Object.entries(owners)) {
    const source = readFileSync(
      resolve(root, `crates/nessa-sdk/src/infrastructure/${file}`),
      "utf8",
    )
    const owned = Number(
      source
        .match(new RegExp(`pub const ${constant}: usize = ([0-9_]+);`))?.[1]
        .replaceAll("_", ""),
    )
    if (
      !Object.hasOwn(rules, name) ||
      !Number.isSafeInteger(owned) ||
      rules[name] !== owned
    )
      throw new Error(`x-mcpServerRules.${name} drifted from the SDK's ${constant}`)
    mcpServerRules[name] = owned
  }
}

const sdkFrames = readFileSync(
  resolve(root, "crates/nessa-sdk/src/infrastructure/session_storage/stream_fact.rs"),
  "utf8",
)
const sdkSource = readFileSync(
  resolve(root, "crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs"),
  "utf8",
)
const ordinaryWire = readFileSync(
  resolve(root, "crates/nessa-protocol/src/protocol/encode.rs"),
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
  // A pairing byte array is the owner's fixed width, so serde refuses any other length.
  const pairingOwner = pairingArrayOwner(node)
  if (pairingOwner) {
    if (!rust) return "number[]"
    externalRustTypes.add(pairingOwner)
    return `[u8; ${rustTypeName(pairingOwner)}::LENGTH]`
  }
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
  // Signed only where the schema admits a negative value, such as a
  // JSON-RPC error code.
  if (node.type === "integer") return rust ? (node.minimum < 0 ? "i64" : "u64") : "number"
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
  "//! Generated from protocol/product/v1.json. Do not edit.\n//! Bounds are validated at the transport boundary; these are payload types only.\n//! Variant names are the schema's wire spellings, so a shared prefix is the wire's.\n#![allow(dead_code, clippy::enum_variant_names)]\nuse serde::{Deserialize, Serialize};\nuse serde_json::Value;\n"
// ConversationErrorCode is named by MCP app audit — a refused call, an app's
// message answered and not sent — and by the product wire.
// It has no payload field, so it is published here with the other outcome
// codes an application module may import.
const sharedOutcomes = new Set([
  "SessionCloseReason",
  "RecordReadErrorCode",
  "CatalogueReadErrorCode",
  "ChangeWatchErrorCode",
  "ChangeWatchEndReason",
  "ConversationErrorCode",
])
// Outcome enums referenced by typed payload fields serialize through Serde.
// Unreferenced code vocabularies and close-policy enums also expose string codes.
const payloadOutcomes = new Set(
  Object.values(schema.$defs).flatMap((definition) =>
    Object.values(definition.properties ?? {}).flatMap((field) =>
      typeof field.$ref === "string" && field.$ref.startsWith("#/$defs/")
        ? [field.$ref.slice("#/$defs/".length)]
        : [],
    ),
  ),
)
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
    // Shared payload outcomes need no separate string-code API.
    if (!sharedOutcomes.has(name) || !payloadOutcomes.has(name) || def["x-close-policy"])
      enumRs += `impl ${name} { pub fn as_str(self) -> &'static str { match self {${def.enum.map((v) => `Self::${pascal(v)} => ${JSON.stringify(v)}`).join(",")} } } }\n`
    if (def["x-close-policy"]) {
      ts += `export const sessionClosePolicy = ${JSON.stringify(def["x-close-policy"])} as const\n`
      enumRs += `impl ${name} { pub fn web_socket_code(self) -> u16 { match self {${def.enum.map((v) => `Self::${pascal(v)} => ${def["x-close-policy"][v].webSocketCode}`).join(",")} } } pub fn retryable(self) -> bool { match self {${def.enum.map((v) => `Self::${pascal(v)} => ${def["x-close-policy"][v].retryable}`).join(",")} } } }\n`
    }
    if (sharedOutcomes.has(name)) contractRs += enumRs
    else rs += enumRs
    continue
  }
  if (def.type === "string" && !def.properties) {
    ts += `export type ${name} = string\n`
    rs += `pub type ${name} = String;\n`
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
      // A required field that may be null must still be present: serde reads
      // a missing `Option` as `None` unless told otherwise.
      rs += `${optional ? '#[serde(default, skip_serializing_if = "Option::is_none")]\n' : Array.isArray(node.type) ? '#[serde(deserialize_with = "Option::deserialize")]\n' : ""}pub ${snake(field)}: ${optional ? `Option<${type(node, true).replace(/^Option<(.*)>$/, "$1")}>` : type(node, true)},\n`
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
// Watch capacity is product policy. The server reads these constants and holds
// no limit of its own; per-connection capacity is the sum of the per-kind limits.
const watchLimits = schema["x-changeWatchLimits"]
const watchLimitNames = [
  "globalOwners",
  "principalOwners",
  "recordTargets",
  "catalogueTargets",
]
for (const name of watchLimitNames) {
  if (
    !watchLimits ||
    !Object.hasOwn(watchLimits, name) ||
    !Number.isSafeInteger(watchLimits[name]) ||
    watchLimits[name] <= 0
  )
    throw new Error(`Invalid change watch limit: ${name}`)
}
if (Object.keys(watchLimits).length !== watchLimitNames.length)
  throw new Error("Invalid change watch limit: unknown policy key")
if (watchLimits.principalOwners > watchLimits.globalOwners)
  throw new Error("Invalid change watch limit: principalOwners exceeds globalOwners")
const connectionWatches = watchLimits.recordTargets + watchLimits.catalogueTargets
const watchId = schema.$defs.ChangeWatchId
if (typeof watchId.pattern !== "string" || !Number.isSafeInteger(watchId.maxLength))
  throw new Error("Invalid change watch ID publication")
rs += `pub const MAX_CHANGE_WATCH_ID_BYTES: usize = ${watchId.maxLength};\n`
// Published so the server can test the identities it mints against the schema.
rs += `pub const CHANGE_WATCH_ID_PATTERN: &str = ${JSON.stringify(watchId.pattern)};\n`
rs += `pub const MAX_GLOBAL_CHANGE_WATCHES: usize = ${watchLimits.globalOwners};\n`
rs += `pub const MAX_PRINCIPAL_CHANGE_WATCHES: usize = ${watchLimits.principalOwners};\n`
rs += `pub const MAX_CONNECTION_RECORD_WATCHES: usize = ${watchLimits.recordTargets};\n`
rs += `pub const MAX_CONNECTION_CATALOGUE_WATCHES: usize = ${watchLimits.catalogueTargets};\n`
rs += `pub const MAX_CONNECTION_CHANGE_WATCHES: usize = ${connectionWatches};\n`
ts += `export const maxChangeWatchIdBytes = ${watchId.maxLength} as const\n`
ts += `export const changeWatchIdPattern = ${JSON.stringify(watchId.pattern)} as const\n`
ts += `export const changeWatchLimits = ${JSON.stringify(watchLimits)} as const\n`
const image = schema.$defs.ImageAttachment.properties
const mcpCall = schema.$defs.McpCallToolParams.properties
const mcpRead = schema.$defs.McpReadResourceParams.properties
const mcpResource = schema.$defs.McpReadResourceResult.properties
const mcpMessage = schema.$defs.McpSendMessageParams.properties
const mcpContext = schema.$defs.McpUpdateModelContextParams.properties
const messageApp = schema.$defs.ConversationMessageApp.properties
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
  // The same gateway limit, read where it bites a client: the gateway takes no
  // WebSocket message longer (`max_message_size`), and its read loop closes the
  // socket on one rather than answering it, so a client must not send one.
  maxRequestFrameBytes: maxOrdinaryResponseBytes,
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
    mcpCall.server["x-utf8MaxBytes"],
    mcpCall.tool["x-utf8MaxBytes"],
    mcpRead.server["x-utf8MaxBytes"],
    mcpMessage.server["x-utf8MaxBytes"],
    mcpContext.server["x-utf8MaxBytes"],
    messageApp.server["x-utf8MaxBytes"],
    messageApp.tool["x-utf8MaxBytes"],
  ]),
  maxUiResourceUriBytes:
    schema.$defs.ConversationMcpTool.properties.resourceUri["x-utf8MaxBytes"],
  mcpAppInstanceIdPattern: schema.$defs.McpAppReference.properties.instanceId.pattern,
  // What an app may send is what its review can show: one bound, stated twice.
  maxMcpArgumentsBytes: agreeing("app arguments and review bytes", [
    mcpCall.argumentsJson["x-utf8MaxBytes"],
    schema.$defs.ConversationPermission.properties.argumentsJson["x-utf8MaxBytes"],
  ]),
  maxMcpResultBytes:
    schema.$defs.McpCallToolResult.properties.resultJson["x-utf8MaxBytes"],
  // An app's message is held to what the person's own may take.
  maxMcpMessageBytes: agreeing("app message and sent message bytes", [
    mcpMessage.text["x-utf8MaxBytes"],
    schema.$defs.ConversationSendParams.properties.text["x-utf8MaxBytes"],
  ]),
  // An empty message is refused at the wire, before anything is recorded.
  minMcpMessageCharacters: mcpMessage.text.minLength,
  // The turn an app's message became is a turn like any other.
  maxExecutionIdBytes: agreeing("execution identity bytes", [
    schema.$defs.McpSendMessageResult.properties.executionId["x-utf8MaxBytes"],
    schema.$defs.ConversationMessage.properties.executionId["x-utf8MaxBytes"],
    messageApp.executionId["x-utf8MaxBytes"],
  ]),
  // An app's context: its text and its structured content, each.
  maxMcpContextBytes: agreeing("app context bytes", [
    mcpContext.text["x-utf8MaxBytes"],
    mcpContext.structuredContentJson["x-utf8MaxBytes"],
  ]),
  maxMcpResourceUriBytes: agreeing("app resource URI bytes", [
    mcpRead.uri["x-utf8MaxBytes"],
    mcpResource.uri["x-utf8MaxBytes"],
    schema.$defs.McpInspectedUi.properties.uri["x-utf8MaxBytes"],
  ]),
  mcpAppMimeType: mcpResource.mimeType.const,
  maxMcpResourceBytes: mcpResource.size.maximum,
  mcpResourceDigestPattern: mcpResource.sha256.pattern,
  mcpResourceTicketPattern: mcpResource.ticket.pattern,
  mcpResourceTicketMs: mcpResource.expiresInMs.const,
  maxMcpCspDomains: agreeing(
    "app CSP list lengths",
    Object.values(schema.$defs.McpUiCsp.properties).map((list) => list.maxItems),
  ),
  maxMcpCspDomainBytes: agreeing(
    "app CSP origin bytes",
    Object.values(schema.$defs.McpUiCsp.properties).map(
      (list) => list.items["x-utf8MaxBytes"],
    ),
  ),
  maxMcpDomainBytes: mcpResource.domain["x-utf8MaxBytes"],
  maxMcpRemoteMessageCharacters:
    schema.$defs.McpRemoteErrorDetails.properties.message.maxLength,
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
  // Each part of an app's context past it is refused at the wire, before
  // anything is recorded; both together are the gateway's to bound.
  "maxMcpContextBytes",
  // An app's message shorter than it is refused at the wire, before anything
  // is recorded; a blank one is the conversation's to refuse, on record.
  "minMcpMessageCharacters",
]) {
  rs += `/// Published bound from the product schema.\npub const ${snake(name).toUpperCase()}: usize = ${bounds[name]};\n`
}
// An app's message past it is refused at the wire, before anything is
// recorded; and the conversation's own input bound is never larger, which the
// gateway's configuration holds to it — so it sits with the contract.
contractRs += `/// Published bound from the product schema.\npub const MAX_MCP_MESSAGE_BYTES: usize = ${bounds.maxMcpMessageBytes};\n`
for (const [name, value] of Object.entries(passiveReadTiming)) {
  rs += `/// Published passive read timing from the product schema, in milliseconds.\npub const PASSIVE_${snake(name).toUpperCase()}: u64 = ${value};\n`
}
// Pure values the gateway's own layers read (the review, the call, the read,
// the ticket), so they sit with the product contract, not the wire. The
// client's allowance and deadlines are the client's alone.
for (const name of ["reviewDeadlineMs", "callTimeoutMs", "readTimeoutMs"]) {
  contractRs += `/// Published MCP App call timing from the product schema, in milliseconds.\npub const MCP_APP_${snake(name).toUpperCase()}: u64 = ${mcpAppCallTiming[name]};\n`
}
// The gateway's own bounds for an inspection; the client's allowance and
// deadline are the client's alone.
for (const name of ["deadlineMs", "maxToolPages", "maxUiReads", "maxConcurrent"]) {
  const type = name === "deadlineMs" ? "u64" : "usize"
  contractRs += `/// Published mcpServers.inspect policy from the product schema${name === "deadlineMs" ? ", in milliseconds" : ""}.\npub const MCP_SERVER_INSPECT_${snake(name).toUpperCase()}: ${type} = ${mcpServerInspect[name]};\n`
}
contractRs += `/// Published lifetime of an MCP App's resource ticket from the product schema, in milliseconds.\npub const MCP_RESOURCE_TICKET_MS: u64 = ${bounds.mcpResourceTicketMs};\n`
ts += `${doc(
  "Passive source and delivery deadlines, plus the client allowance. The minimum request deadline is their sum; clients raise shorter configured timeouts to this floor.",
)}export const passiveReadTiming = ${JSON.stringify(passiveReadTiming)} as const\n`
ts += `${doc(
  "How long an MCP App's calls can take the gateway: a destructive tool's review waits up to reviewDeadlineMs for the person, then the call itself up to callTimeoutMs; a resource read up to readTimeoutMs; clientAllowanceMs covers audit writes, the response and scheduling. The client waits callDeadlineMs for mcp.callTool, and for mcp.readResource too, since the gateway may open the conversation first.",
)}export const mcpAppCallTiming = ${JSON.stringify(mcpAppCallTiming)} as const\n`
ts += `${doc(
  "How mcpServers.inspect is bounded: one inspection runs at most deadlineMs, reads at most maxToolPages pages of tools and maxUiReads UI resources, and at most maxConcurrent run at once. The client waits requestDeadlineMs, the deadline plus clientAllowanceMs for stopping the server, the audit records and the response.",
)}export const mcpServerInspect = ${JSON.stringify(mcpServerInspect)} as const\n`
ts += `${doc(
  "The SDK's rules for a stored MCP server, as x-mcpServerRules publishes them: at most maxServers servers, the managed one included; a name of 1 to nameMaxBytes bytes; at most maxArgs arguments of at most argMaxBytes bytes each; a variable name of 1 to environmentNameMaxBytes bytes. The gateway refuses past them (mcp_servers_invalid); a client may refuse early by reading these.",
)}export const mcpServerRules = ${JSON.stringify(mcpServerRules)} as const\n`
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
  [`${pairingDirectory}/wire_values.rs`, pairing.rust],
  [
    "protocol/product/pairing-values.generated.json",
    await format(JSON.stringify(pairing.schema), {
      ...(await resolveConfig(resolve(root, "prettier.config.js"))),
      parser: "json",
    }),
  ],
  ...(schemaOutput === undefined ? [] : [["protocol/product/v1.json", schemaOutput]]),
  ["packages/nessa-client/src/generated/product.ts", ts],
  ["crates/nessa-protocol/src/product/generated.rs", formatted.stdout],
  ["crates/nessa-protocol/src/product_contract/generated.rs", formattedContract.stdout],
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
