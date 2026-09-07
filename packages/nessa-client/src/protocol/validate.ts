import type { EventFrame, Frame, ResFrame } from "./types.js"
import type { ProductSessionReady, SessionChallenge } from "./product-types.js"

const RUNTIME_STATUSES = new Set(["ready", "starting", "unavailable", "error"])

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0
}

function isPositiveInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 1
}

function isNonNegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
}

export function assertSessionChallenge(value: unknown): SessionChallenge {
  if (
    !isRecord(value) ||
    !isPositiveInteger(value.minVersion) ||
    !isPositiveInteger(value.maxVersion)
  ) {
    throw new Error("session.challenge has invalid version range")
  }
  if (value.minVersion > value.maxVersion || !isNonEmptyString(value.nonce)) {
    throw new Error("session.challenge is invalid")
  }
  if (!isNonNegativeInteger(value.expiresAt)) {
    throw new Error("session.challenge has invalid expiresAt")
  }
  return value as unknown as SessionChallenge
}

export function assertProductSessionReady(value: unknown): ProductSessionReady {
  if (!isRecord(value) || value.version !== 1) {
    throw new Error("session response has unsupported version")
  }
  for (const field of [
    "gatewayId",
    "principalId",
    "organizationId",
    "membershipId",
    "credentialId",
    "audienceId",
  ] as const) {
    if (!isNonEmptyString(value[field]))
      throw new Error(`session response missing ${field}`)
  }
  if (value.expiresAt !== null && !isNonNegativeInteger(value.expiresAt)) {
    throw new Error("session response has invalid expiresAt")
  }
  if (!Array.isArray(value.grants)) {
    throw new Error("session response has invalid grants")
  }
  for (const grantValue of value.grants) {
    if (!isRecord(grantValue) || !isNonEmptyString(grantValue.action)) {
      throw new Error("session response has invalid grant")
    }
    const resource = grantValue.resource
    if (
      !isRecord(resource) ||
      !isNonEmptyString(resource.organizationId) ||
      resource.organizationId !== value.organizationId ||
      !isNonEmptyString(resource.id)
    ) {
      throw new Error("session response has invalid grant resource")
    }
  }
  if (!Array.isArray(value.methods)) {
    throw new Error("session response has invalid methods")
  }
  const methods = new Set<string>()
  for (const method of value.methods) {
    if (!isNonEmptyString(method) || methods.has(method)) {
      throw new Error("session response has invalid methods")
    }
    methods.add(method)
  }
  return value as unknown as ProductSessionReady
}

function isRuntimeStatus(
  value: unknown,
): value is import("./types.js").HealthResult["runtimeStatus"] {
  return typeof value === "string" && RUNTIME_STATUSES.has(value)
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

/** Validate a conversation.echo result payload. */
export function assertEchoResult(value: unknown): import("./types.js").EchoResult {
  if (!isRecord(value) || !isNonEmptyString(value.text)) {
    throw new Error("echo response is not a valid EchoResult")
  }
  return value as unknown as import("./types.js").EchoResult
}
