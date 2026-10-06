import {
  bounds,
  type McpCallToolResult,
  type McpReadResourceResult,
  type McpRemoteErrorDetails,
  type McpAppReference,
  type McpSendMessageResult,
  type McpUiCsp,
  type McpUiPermissions,
} from "../generated/product.js"
import { wellFormedText } from "./unicode.js"

/**
 * Bounds the protocol schema puts on an MCP App's calls, named here for what
 * they mean and generated from the schema, so no number is copied.
 */
/** `McpCallToolParams.argumentsJson` maximum, in UTF-8 bytes: the most a review shows. */
export const MAX_MCP_ARGUMENTS_BYTES = bounds.maxMcpArgumentsBytes
/** `McpCallToolResult.resultJson` maximum, in UTF-8 bytes. */
export const MAX_MCP_RESULT_BYTES = bounds.maxMcpResultBytes
/** `McpReadResourceResult.size` maximum: the most one app resource holds. */
export const MAX_MCP_RESOURCE_BYTES = bounds.maxMcpResourceBytes
/** `McpSendMessageParams.text` maximum, in UTF-8 bytes: the most a message an app sends may take. */
export const MAX_MCP_MESSAGE_BYTES = bounds.maxMcpMessageBytes
/**
 * `McpUpdateModelContextParams.text` and `.structuredContentJson` maximum,
 * each, in UTF-8 bytes. Whether the two fit together is the gateway's to judge
 * (`mcp_request_too_large`).
 */
export const MAX_MCP_CONTEXT_BYTES = bounds.maxMcpContextBytes

const utf8 = new TextEncoder()

/**
 * What an MCP App may send its server, held to the schema's bounds: the one
 * statement of them. `McpAppsApi` refuses a request past them before sending
 * anything, and a host may ask first, so it can refuse the app's request
 * itself rather than read a `TypeError` whose cause it cannot tell. Each
 * must also be Unicode text (`wellFormedText`): this client refuses a lone
 * surrogate before send. If one still reaches the gateway, the frame is
 * answered `invalid_request` when the envelope parser reads one JSON object,
 * no decoded envelope name appears twice, `type` is `req`, and `id` is one
 * Unicode string of 1 to 256 bytes. An envelope name that is not Unicode is
 * not a second name, a repeated name inside a nested value still leaves that
 * id, and a frame deeper than 127 containers is not read (#403). What the
 * arguments decode to — an object, its strings — is the gateway's to judge,
 * and it answers `invalid_request`.
 *
 * Each answers the problem in words, or `undefined` within bounds.
 */
export const mcpAppRequestProblem = {
  /** A tool's name: 1 to `maxMcpNameBytes` UTF-8 bytes of Unicode. */
  tool: (tool: string): string | undefined =>
    !boundedName(tool, bounds.maxMcpNameBytes)
      ? `Tool must contain 1-${bounds.maxMcpNameBytes} UTF-8 bytes`
      : !wellFormedText(tool)
        ? "Tool must be Unicode text"
        : undefined,
  /** A resource's URI: 1 to `maxMcpResourceUriBytes` UTF-8 bytes of Unicode. */
  uri: (uri: string): string | undefined =>
    !boundedName(uri, bounds.maxMcpResourceUriBytes)
      ? `Resource URI must contain 1-${bounds.maxMcpResourceUriBytes} UTF-8 bytes`
      : !wellFormedText(uri)
        ? "Resource URI must be Unicode text"
        : undefined,
  /**
   * A tool's arguments, encoded: at most `MAX_MCP_ARGUMENTS_BYTES` UTF-8
   * bytes of Unicode text. The byte bound is checked first, so a text far
   * past it is refused without being encoded (`refuses arguments and a name
   * far past their bound without encoding them`).
   */
  argumentsJson: (argumentsJson: string): string | undefined =>
    !withinUtf8Bytes(argumentsJson, MAX_MCP_ARGUMENTS_BYTES)
      ? `Arguments must contain at most ${MAX_MCP_ARGUMENTS_BYTES} UTF-8 bytes`
      : !wellFormedText(argumentsJson)
        ? "Arguments must be Unicode text"
        : undefined,
  /**
   * A message an app sends: `minMcpMessageCharacters` (the schema's
   * `minLength`, in code points) to `MAX_MCP_MESSAGE_BYTES` UTF-8 bytes of
   * Unicode. Whether it is blank, whitespace only, is the conversation's to
   * judge (`invalid_request`, on record). The byte bound is checked first, so
   * a text far past it is refused without being encoded or read character by
   * character (`K1: refuses a text far past its bound without reading it`).
   * Anything but a string — a `String` object, an array, `null` — is refused
   * too: this is the check `sendMessage` makes.
   */
  message: (text: unknown): string | undefined =>
    typeof text !== "string"
      ? "Message must be a string"
      : !withinUtf8Bytes(text, MAX_MCP_MESSAGE_BYTES) ||
          !atLeastCodePoints(text, bounds.minMcpMessageCharacters)
        ? `Message must contain ${bounds.minMcpMessageCharacters} character to ${MAX_MCP_MESSAGE_BYTES} UTF-8 bytes`
        : !wellFormedText(text)
          ? "Message must be Unicode text"
          : undefined,
  /**
   * The context an app gives the model: a plain object of its own `text` and
   * `structuredContentJson` and nothing else, each part a string of at most
   * `MAX_MCP_CONTEXT_BYTES` UTF-8 bytes of Unicode, or absent. An array, or an
   * object with another key, is refused rather than sent as an update with
   * neither part, which would clear what the mount gave. Whether the two parts
   * fit together, and whether the structure is one JSON object, are the
   * gateway's to judge (`mcp_request_too_large`, `invalid_request`).
   */
  context: (context: unknown): string | undefined => {
    const read = mcpAppModelContext(context)
    return typeof read === "string" ? read : undefined
  },
} as const

/**
 * Whether `text` is at most `max` UTF-8 bytes. Every UTF-16 code unit is at
 * least one UTF-8 byte (a lone surrogate encodes as three), so a text of more
 * units than `max` is past it without being encoded.
 */
function withinUtf8Bytes(text: string, max: number): boolean {
  return text.length <= max && utf8.encode(text).byteLength <= max
}

/** Whether `text` holds at least `min` code points, counting no further than `min`. */
function atLeastCodePoints(text: string, min: number): boolean {
  let count = 0
  if (count >= min) return true
  for (const _ of text) if (++count >= min) return true
  return false
}

/** What an app gives the model: either part, both, or neither, which clears what its mount gave. */
export interface McpAppModelContext {
  /** The context as text. Empty is none. */
  readonly text?: string
  /** The context's structured content: one JSON object, encoded. */
  readonly structuredContentJson?: string
}

const contextParts = ["text", "structuredContentJson"] as const

/** An object made as `{}` or `Object.create(null)` is: no array, class instance or other built-in. */
function plainObject(value: unknown): value is object {
  if (!value || typeof value !== "object") return false
  const prototype: unknown = Object.getPrototypeOf(value)
  return prototype === Object.prototype || prototype === null
}

/**
 * A context as the wire carries it — each part given, neither for a clear —
 * or, outside the bounds {@link mcpAppRequestProblem}`.context` states, the
 * problem in words. Each part is read once, from the object's own keys only
 * (`Object.hasOwn`): one it would only inherit is not part of it, and what is
 * checked is what is sent.
 */
export function mcpAppModelContext(context: unknown): McpAppModelContext | string {
  if (!plainObject(context))
    return "Context must be an object of text and structuredContentJson"
  if (
    Reflect.ownKeys(context).some(
      (key) => !(contextParts as readonly PropertyKey[]).includes(key),
    )
  )
    return "Context may hold only text and structuredContentJson"
  const parts: { text?: string; structuredContentJson?: string } = {}
  for (const part of contextParts) {
    const value: unknown = Object.hasOwn(context, part)
      ? (context as Record<string, unknown>)[part]
      : undefined
    if (value === undefined) continue
    if (typeof value !== "string") return `Context ${part} must be a string`
    if (!withinUtf8Bytes(value, MAX_MCP_CONTEXT_BYTES))
      return `Context ${part} must contain at most ${MAX_MCP_CONTEXT_BYTES} UTF-8 bytes`
    if (!wellFormedText(value)) return `Context ${part} must be Unicode text`
    parts[part] = value
  }
  return parts
}

/**
 * The answer to `mcp.sendMessage`: the turn the message became, an execution
 * id of 1 to `maxExecutionIdBytes` UTF-8 bytes of Unicode, and nothing else.
 */
export function mcpSendMessageResult(value: unknown): McpSendMessageResult {
  const item = object(value, ["executionId"], "message result")
  if (
    !boundedName(item.executionId, bounds.maxExecutionIdBytes) ||
    !wellFormedText(item.executionId)
  )
    throw new Error("Invalid message executionId")
  return { executionId: item.executionId }
}

const instanceIdPattern = new RegExp(bounds.mcpAppInstanceIdPattern)
const digestPattern = new RegExp(bounds.mcpResourceDigestPattern)
const ticketPattern = new RegExp(bounds.mcpResourceTicketPattern)
const cspKeys = ["connectDomains", "resourceDomains", "frameDomains", "baseUriDomains"]
const permissionKeys = ["camera", "microphone", "geolocation", "clipboardWrite"]
const resourceKeys = [
  "uri",
  "mimeType",
  "size",
  "sha256",
  "ticket",
  "expiresInMs",
  "csp",
  "permissions",
  "domain",
  "prefersBorder",
]

/**
 * A string of 1 to `max` UTF-8 bytes: the schema's `minLength: 1` and
 * `x-utf8MaxBytes`. A string of more than `max` UTF-16 units is past the
 * bound without being encoded (`withinUtf8Bytes`).
 */
export function boundedName(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length > 0 && withinUtf8Bytes(value, max)
}

/** A plain object holding no field but `keys`. */
export function object(value: unknown, keys: readonly string[], what: string) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error(`Invalid ${what}`)
  if (Object.keys(value).some((key) => !keys.includes(key)))
    throw new Error(`${what} has unknown fields`)
  return value as Record<string, unknown>
}

/** A resource ticket's shape: 43 base64url characters. */
export function validResourceTicket(value: unknown): value is string {
  return typeof value === "string" && ticketPattern.test(value)
}
/** A resource digest's shape: 64 lowercase hexadecimal digits of SHA-256. */
export function validResourceDigest(value: unknown): value is string {
  return typeof value === "string" && digestPattern.test(value)
}
/** A resource's size: a whole number of bytes, 0 to 4 MiB. */
export function validResourceSize(value: unknown): value is number {
  return (
    Number.isSafeInteger(value) &&
    (value as number) >= 0 &&
    (value as number) <= MAX_MCP_RESOURCE_BYTES
  )
}

const appFields = ["executionId", "toolId", "instanceId"] as const

/**
 * An app reference as the wire carries it, or the problem in words. Each
 * field is read once, from the object's own keys only (`Object.hasOwn`): one
 * it would only inherit is not part of it, and what is checked is what is
 * sent. A field must be Unicode; the byte bound is checked first, so a text
 * far past it is refused without being encoded.
 */
export function mcpAppReference(app: unknown): McpAppReference | string {
  if (!app || typeof app !== "object" || Array.isArray(app))
    return "an app must be an object"
  if (
    Reflect.ownKeys(app).some(
      (key) => !(appFields as readonly PropertyKey[]).includes(key),
    )
  )
    return "an app has unknown fields"
  const record = app as Record<string, unknown>
  const executionId = Object.hasOwn(app, "executionId") ? record.executionId : undefined
  const toolId = Object.hasOwn(app, "toolId") ? record.toolId : undefined
  const instanceId = Object.hasOwn(app, "instanceId") ? record.instanceId : undefined
  if (!boundedName(executionId, 256)) return "an app's execution ID must be 1-256 bytes"
  if (!wellFormedText(executionId)) return "an app's execution ID must be Unicode text"
  if (!boundedName(toolId, 256)) return "an app's tool call ID must be 1-256 bytes"
  if (!wellFormedText(toolId)) return "an app's tool call ID must be Unicode text"
  if (typeof instanceId !== "string")
    return "an app's instance ID must be a canonical lowercase UUID"
  if (!wellFormedText(instanceId)) return "an app's instance ID must be Unicode text"
  if (!instanceIdPattern.test(instanceId))
    return "an app's instance ID must be a canonical lowercase UUID"
  return { executionId, toolId, instanceId }
}

/** Why a value is not an app reference the gateway would take, or undefined when it is. */
export function mcpAppReferenceProblem(app: unknown): string | undefined {
  const read = mcpAppReference(app)
  return typeof read === "string" ? read : undefined
}

/**
 * The answer to `mcp.callTool`: the server's `CallToolResult`, encoded as one
 * JSON object within the schema's byte bound (its two-character minimum is the
 * shortest object, so being one is the whole check). A result that is not one is no
 * answer this gateway could have given, and is refused rather than handed to
 * the app.
 */
export function mcpCallToolResult(value: unknown): McpCallToolResult {
  const item = object(value, ["resultJson"], "tool result")
  const resultJson = item.resultJson
  if (
    typeof resultJson !== "string" ||
    utf8.encode(resultJson).byteLength > MAX_MCP_RESULT_BYTES
  )
    throw new Error("Invalid tool resultJson")
  let result: unknown
  try {
    result = JSON.parse(resultJson)
  } catch {
    throw new Error("Tool resultJson is not JSON")
  }
  if (!result || typeof result !== "object" || Array.isArray(result))
    throw new Error("Tool resultJson is not one JSON object")
  return { resultJson }
}

/** An app's CSP as the schema bounds it: each list of origins within its count and byte bounds. */
export function csp(value: unknown): McpUiCsp {
  const item = object(value, cspKeys, "app CSP")
  const lists = cspKeys.map((key) => {
    const list = item[key]
    if (
      !Array.isArray(list) ||
      list.length > bounds.maxMcpCspDomains ||
      list.some((origin) => !boundedName(origin, bounds.maxMcpCspDomainBytes))
    )
      throw new Error(`Invalid app CSP ${key}`)
    return [...(list as string[])]
  })
  const [connectDomains, resourceDomains, frameDomains, baseUriDomains] = lists as [
    string[],
    string[],
    string[],
    string[],
  ]
  return { connectDomains, resourceDomains, frameDomains, baseUriDomains }
}

/** What an app asked of the host: every permission a boolean. */
export function permissions(value: unknown): McpUiPermissions {
  const item = object(value, permissionKeys, "app permissions")
  for (const key of permissionKeys)
    if (typeof item[key] !== "boolean") throw new Error(`Invalid app permission ${key}`)
  const { camera, microphone, geolocation, clipboardWrite } =
    item as unknown as McpUiPermissions
  return { camera, microphone, geolocation, clipboardWrite }
}

/**
 * The answer to `mcp.readResource`, held to every bound and constant the schema
 * states. It must name the resource that was asked for — the gateway answers
 * the URI as it was given — and say it is an MCP App's HTML with the one ticket
 * lifetime there is. `domain` and `prefersBorder` are absent when the app did
 * not say; null is not absent, and is refused. Messages here never include the
 * ticket.
 */
export function mcpReadResourceResult(
  value: unknown,
  uri: string,
): McpReadResourceResult {
  const item = object(value, resourceKeys, "resource answer")
  if (item.uri !== uri) throw new Error("Resource answer names another resource")
  if (item.mimeType !== bounds.mcpAppMimeType)
    throw new Error("Resource answer is not an MCP App's HTML")
  if (!validResourceSize(item.size)) throw new Error("Invalid resource size")
  if (!validResourceDigest(item.sha256)) throw new Error("Invalid resource sha256")
  if (!validResourceTicket(item.ticket))
    throw new Error("Resource answer has no usable ticket")
  if (item.expiresInMs !== bounds.mcpResourceTicketMs)
    throw new Error("Invalid resource ticket lifetime")
  if (item.domain !== undefined && !boundedName(item.domain, bounds.maxMcpDomainBytes))
    throw new Error("Invalid resource domain")
  if (item.prefersBorder !== undefined && typeof item.prefersBorder !== "boolean")
    throw new Error("Invalid resource prefersBorder")
  return {
    uri,
    mimeType: bounds.mcpAppMimeType,
    size: item.size,
    sha256: item.sha256,
    ticket: item.ticket,
    expiresInMs: bounds.mcpResourceTicketMs,
    csp: csp(item.csp),
    permissions: permissions(item.permissions),
    ...(item.domain === undefined ? {} : { domain: item.domain as string }),
    ...(item.prefersBorder === undefined
      ? {}
      : { prefersBorder: item.prefersBorder as boolean }),
  }
}

/**
 * The server's own JSON-RPC error, from the details of an `mcp_remote_error`
 * refusal, or undefined when they are absent or not that shape. Details that
 * are not are the refusal without its explanation, not a different refusal.
 */
export function mcpRemoteErrorDetails(
  details: unknown,
): McpRemoteErrorDetails | undefined {
  if (!details || typeof details !== "object" || Array.isArray(details)) return undefined
  if (Object.keys(details).some((key) => key !== "code" && key !== "message"))
    return undefined
  const { code, message } = details as Record<string, unknown>
  if (
    !Number.isSafeInteger(code) ||
    typeof message !== "string" ||
    // `maxLength` counts code points. Its 2048-byte bound is four bytes for
    // each of them, so this is the whole check.
    [...message].length > bounds.maxMcpRemoteMessageCharacters
  )
    return undefined
  return { code: code as number, message }
}
