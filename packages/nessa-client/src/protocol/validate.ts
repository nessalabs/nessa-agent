import type {
  ConnectChallenge,
  EventFrame,
  Frame,
  HelloOk,
  ResFrame,
  Scope,
} from "./types.js"

const RUNTIME_STATUSES = new Set(["ready", "starting", "unavailable", "error"])
const SCOPES = new Set<Scope>(["server.read"])

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0
}

function isPositiveInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 1
}

function isRuntimeStatus(value: unknown): value is HelloOk["runtimeStatus"] {
  return typeof value === "string" && RUNTIME_STATUSES.has(value)
}

function isScope(value: unknown): value is Scope {
  return typeof value === "string" && SCOPES.has(value as Scope)
}

function isScopeList(value: unknown): value is Scope[] {
  if (!Array.isArray(value) || value.length === 0) return false
  const seen = new Set<string>()
  for (const item of value) {
    if (!isScope(item) || seen.has(item)) return false
    seen.add(item)
  }
  return true
}

function hasOnlyKeys(
  value: Record<string, unknown>,
  allowed: readonly string[],
): boolean {
  const keys = Object.keys(value)
  return keys.length <= allowed.length && keys.every((key) => allowed.includes(key))
}

/** Parse and validate a response frame from untrusted wire JSON. */
export function parseResponseFrame(value: unknown): ResFrame | null {
  if (!isRecord(value) || value.type !== "res") return null
  if (!hasOnlyKeys(value, ["type", "id", "ok", "payload", "error"])) return null
  if (!isNonEmptyString(value.id) || typeof value.ok !== "boolean") return null

  if (value.ok) {
    if (!("payload" in value) || "error" in value) return null
    return value as unknown as ResFrame
  }

  if ("payload" in value || !isRecord(value.error)) return null
  if (!hasOnlyKeys(value.error, ["code", "message", "details"])) return null
  if (!isNonEmptyString(value.error.code)) return null
  if (typeof value.error.message !== "string") return null
  return value as unknown as ResFrame
}

/** Parse and validate an event frame from untrusted wire JSON. */
export function parseEventFrame(value: unknown): EventFrame | null {
  if (!isRecord(value) || value.type !== "event") return null
  if (!hasOnlyKeys(value, ["type", "event", "payload", "seq", "stateVersion"]))
    return null
  if (!isNonEmptyString(value.event)) return null
  if (!("payload" in value)) return null
  if (typeof value.seq !== "number" || value.seq < 0 || !Number.isInteger(value.seq)) {
    return null
  }
  if (
    typeof value.stateVersion !== "number" ||
    value.stateVersion < 0 ||
    !Number.isInteger(value.stateVersion)
  ) {
    return null
  }
  return value as unknown as EventFrame
}

/** Parse WebSocket text into a validated frame, or null when invalid. */
export function parseWireMessage(raw: string): Frame | null {
  let value: unknown
  try {
    value = JSON.parse(raw)
  } catch {
    return null
  }

  return parseResponseFrame(value) ?? parseEventFrame(value)
}

/** Validate a successful connect payload before exposing it as session state. */
export function assertHelloOk(value: unknown): HelloOk {
  if (!isRecord(value)) {
    throw new Error("connect response is not an object")
  }
  if (value.protocol !== 1) {
    throw new Error("connect response has unsupported protocol version")
  }
  if (!isScopeList(value.scopes)) {
    throw new Error("connect response has invalid scopes")
  }
  if (!isNonEmptyString(value.serverVersion)) {
    throw new Error("connect response missing serverVersion")
  }
  if (!isRuntimeStatus(value.runtimeStatus)) {
    throw new Error("connect response has invalid runtimeStatus")
  }
  if (!isRecord(value.policy) || !isPositiveInteger(value.policy.maxPayloadBytes)) {
    throw new Error("connect response has invalid policy.maxPayloadBytes")
  }
  return value as unknown as HelloOk
}

/** Validate a connect.challenge event payload. */
export function assertConnectChallenge(value: unknown): ConnectChallenge {
  if (!isRecord(value)) {
    throw new Error("connect.challenge payload is not an object")
  }
  if (!isNonEmptyString(value.nonce)) {
    throw new Error("connect.challenge missing nonce")
  }
  if (value.protocol !== 1) {
    throw new Error("connect.challenge has unsupported protocol version")
  }
  return value as unknown as ConnectChallenge
}

/** Validate a server.health result payload. */
export function assertHealthResult(value: unknown): import("./types.js").HealthResult {
  if (!isRecord(value) || typeof value.ok !== "boolean") {
    throw new Error("health response is not a valid HealthResult")
  }
  if (!isRuntimeStatus(value.runtimeStatus)) {
    throw new Error("health response has invalid runtimeStatus")
  }
  if (
    typeof value.uptimeMs !== "number" ||
    value.uptimeMs < 0 ||
    !Number.isInteger(value.uptimeMs)
  ) {
    throw new Error("health response has invalid uptimeMs")
  }
  return value as unknown as import("./types.js").HealthResult
}
