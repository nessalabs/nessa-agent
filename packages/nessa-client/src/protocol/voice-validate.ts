/**
 * The answers of the `voice.*` methods, held to the schema's shape before
 * anyone believes them.
 */
import { VoiceTransport, type VoiceCall, type VoiceStatus } from "../generated/product.js"

function invalid(what: string): never {
  throw new Error(`voice response has invalid ${what}`)
}

function record(value: unknown, allowed: readonly string[]): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    invalid("shape")
  const item = value as Record<string, unknown>
  for (const key of Object.keys(item)) if (!allowed.includes(key)) invalid(key)
  return item
}

function own(item: Record<string, unknown>, key: string): unknown {
  return Object.hasOwn(item, key) ? item[key] : undefined
}

function text(value: unknown, most: number, what: string): string {
  if (typeof value !== "string" || value.length === 0 || value.length > most)
    invalid(what)
  return value
}

export function voiceStatus(value: unknown): VoiceStatus {
  const item = record(value, ["keySaved", "model"])
  const keySaved = own(item, "keySaved")
  if (typeof keySaved !== "boolean") invalid("keySaved")
  return { keySaved, model: text(own(item, "model"), 128, "model") }
}

const transports = new Set<string>(Object.values(VoiceTransport))

export function voiceCall(value: unknown): VoiceCall {
  const item = record(value, [
    "transport",
    "callUrl",
    "clientSecret",
    "expiresAt",
    "model",
  ])
  const transport = own(item, "transport")
  if (typeof transport !== "string" || !transports.has(transport)) invalid("transport")
  const expiresAt = own(item, "expiresAt")
  if (!Number.isSafeInteger(expiresAt) || (expiresAt as number) < 0) invalid("expiresAt")
  return {
    transport: transport as VoiceTransport,
    callUrl: text(own(item, "callUrl"), 2048, "callUrl"),
    clientSecret: text(own(item, "clientSecret"), 4096, "clientSecret"),
    expiresAt: expiresAt as number,
    model: text(own(item, "model"), 128, "model"),
  }
}
