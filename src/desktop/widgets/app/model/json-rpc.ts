/**
 * JSON-RPC 2.0 as an MCP App's frame speaks it (MCP Apps, *Communication
 * Protocol*), read from data the window does not trust.
 *
 * Everything that arrives is parsed into a **copy** made here — plain
 * objects, arrays and JSON scalars, rebuilt field by field — and only the
 * copy is used afterwards: the original is never read twice, so nothing can
 * change between the check and the use. The copy is bounded (depth, nodes,
 * string length), so a hostile frame cannot make the window hold more than
 * that for it.
 */

/** A request's id: a JSON-RPC string or number, never `null`. */
export type RequestId = string | number

/** A JSON value, as the copy holds it. */
export type Json = null | boolean | number | string | readonly Json[] | JsonObject
export interface JsonObject {
  readonly [key: string]: Json
}

/** The JSON-RPC error codes the host answers with. */
export const errorCodes = {
  parse: -32700,
  invalidRequest: -32600,
  methodNotFound: -32601,
  invalidParams: -32602,
  internal: -32603,
  /** MCP Apps' implementation-defined code: denied, or failed. */
  refused: -32000,
  /** Asked before `ui/notifications/initialized`. */
  notInitialized: -32002,
} as const

export type ErrorCode = (typeof errorCodes)[keyof typeof errorCodes]

/** What one message from a frame is, once read. */
export type Envelope =
  | {
      readonly kind: "request"
      readonly id: RequestId
      readonly method: string
      readonly params: JsonObject
    }
  | {
      readonly kind: "notification"
      readonly method: string
      readonly params: JsonObject
    }
  | { readonly kind: "result"; readonly id: RequestId; readonly result: JsonObject }
  | { readonly kind: "error"; readonly id: RequestId; readonly code: number }
  /** Not a JSON-RPC 2.0 message; `id` when one could be read, to answer it. */
  | { readonly kind: "malformed"; readonly id: RequestId | null }

/** How much of a frame's data one message may make the window hold. */
export interface CopyBounds {
  readonly depth: number
  readonly nodes: number
  readonly stringLength: number
}

/** The bounds every message is read under: generous for any app, finite for a hostile one. */
export const messageBounds: CopyBounds = {
  depth: 64,
  nodes: 100_000,
  stringLength: 4 * 1024 * 1024,
}

const methodLength = 256
const idLength = 256

/**
 * A copy of `value` as JSON, or `undefined` when it is not JSON (a function,
 * `undefined`, a non-finite number, a typed array, a cycle) or exceeds
 * `bounds`. Own enumerable string keys only; `__proto__` is copied as an own
 * key, never as the copy's prototype.
 */
export function copyJson(
  value: unknown,
  bounds: CopyBounds = messageBounds,
): Json | undefined {
  let nodes = 0
  const copy = (item: unknown, depth: number): Json | undefined => {
    if (++nodes > bounds.nodes || depth > bounds.depth) return undefined
    if (item === null || typeof item === "boolean") return item
    if (typeof item === "number") return Number.isFinite(item) ? item : undefined
    if (typeof item === "string")
      return item.length <= bounds.stringLength ? item : undefined
    if (typeof item !== "object") return undefined
    if (Array.isArray(item)) {
      const out: Json[] = []
      for (let at = 0; at < item.length; at++) {
        const entry = copy(item[at], depth + 1)
        if (entry === undefined) return undefined
        out.push(entry)
      }
      return out
    }
    const prototype = Object.getPrototypeOf(item)
    if (prototype !== Object.prototype && prototype !== null) return undefined
    const out: Record<string, Json> = {}
    for (const key of Object.keys(item)) {
      if (key.length > bounds.stringLength) return undefined
      const entry = copy((item as Record<string, unknown>)[key], depth + 1)
      if (entry === undefined) return undefined
      Object.defineProperty(out, key, {
        value: entry,
        enumerable: true,
        writable: true,
        configurable: true,
      })
    }
    return out
  }
  return copy(value, 0)
}

/** Whether a copied value is a JSON object (not an array, not a scalar). */
export function isObject(value: Json | undefined): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

/** The value `object` holds under `key` itself — never one it inherits. */
export function field(object: JsonObject, key: string): Json | undefined {
  return Object.hasOwn(object, key) ? object[key] : undefined
}

function readId(value: Json | undefined): RequestId | null {
  if (typeof value === "string") return value.length <= idLength ? value : null
  if (typeof value === "number") return Number.isFinite(value) ? value : null
  return null
}

const noParams: JsonObject = Object.freeze({})

/**
 * Reads one message. The whole of `data` is copied first, under `bounds`;
 * everything after reads the copy.
 */
export function readEnvelope(
  data: unknown,
  bounds: CopyBounds = messageBounds,
): Envelope {
  const message = copyJson(data, bounds)
  if (!isObject(message) || field(message, "jsonrpc") !== "2.0")
    return { kind: "malformed", id: null }
  const rawId = field(message, "id")
  const id = readId(rawId)
  const hasId = rawId !== undefined
  // An id that is present and not a valid one cannot be answered under it.
  if (hasId && id === null) return { kind: "malformed", id: null }
  const method = field(message, "method")
  if (method !== undefined) {
    if (typeof method !== "string" || method.length === 0 || method.length > methodLength)
      return { kind: "malformed", id }
    const params = field(message, "params")
    if (params !== undefined && !isObject(params)) return { kind: "malformed", id }
    const read = params ?? noParams
    return id === null
      ? { kind: "notification", method, params: read }
      : { kind: "request", id, method, params: read }
  }
  if (id === null) return { kind: "malformed", id: null }
  const result = field(message, "result")
  const error = field(message, "error")
  if (isObject(result) && error === undefined) return { kind: "result", id, result }
  if (isObject(error) && result === undefined) {
    const code = field(error, "code")
    if (typeof code === "number" && Number.isInteger(code))
      return { kind: "error", id, code }
  }
  return { kind: "malformed", id }
}

/** What the host posts to a frame. */
export type Outgoing =
  | { readonly jsonrpc: "2.0"; readonly id: RequestId; readonly result: JsonObject }
  | {
      readonly jsonrpc: "2.0"
      readonly id: RequestId
      /** One of `errorCodes`, or a server's own code passed on (`relayError`). */
      readonly error: { readonly code: number; readonly message: string }
    }
  | { readonly jsonrpc: "2.0"; readonly method: string; readonly params: JsonObject }
  | {
      readonly jsonrpc: "2.0"
      readonly id: RequestId
      readonly method: string
      readonly params: JsonObject
    }

export const reply = (id: RequestId, result: JsonObject): Outgoing => ({
  jsonrpc: "2.0",
  id,
  result,
})

export const refuse = (id: RequestId, code: ErrorCode, message: string): Outgoing => ({
  jsonrpc: "2.0",
  id,
  error: { code, message },
})

/**
 * An error the app's own server answered with, passed on as it came: its code
 * is any integer, of either sign (JSON-RPC 2.0, *Error object*), and the
 * gateway has already bounded its message (`McpRemoteErrorDetails`).
 */
export const relayError = (
  id: RequestId,
  error: { readonly code: number; readonly message: string },
): Outgoing => ({
  jsonrpc: "2.0",
  id,
  error: { code: error.code, message: error.message },
})

export const notify = (method: string, params: JsonObject): Outgoing => ({
  jsonrpc: "2.0",
  method,
  params,
})

export const request = (id: RequestId, method: string, params: JsonObject): Outgoing => ({
  jsonrpc: "2.0",
  id,
  method,
  params,
})
