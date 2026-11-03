/**
 * The answer of every `logins.*` method, held to the schema's shape before
 * anyone believes it.
 */
import { LoginAgent, type LoginsStatus, type SavedLogin } from "../generated/product.js"

function invalid(what: string): never {
  throw new Error(`logins response has invalid ${what}`)
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

const agents: readonly LoginAgent[] = [
  LoginAgent.Claude,
  LoginAgent.Codex,
  LoginAgent.Opencode,
]
const peerName = /^peer:[0-9a-f]{64}$/
const MAX_DEVICES = 256

export function loginsStatus(value: unknown): LoginsStatus {
  const item = record(value, ["logins", "devicesOn"])
  const logins = own(item, "logins")
  if (!Array.isArray(logins) || logins.length !== agents.length) invalid("logins")
  const read: SavedLogin[] = agents.map((agent, index) => {
    const login = record(logins[index], ["agent", "saved"])
    if (own(login, "agent") !== agent) invalid("agent")
    const saved = own(login, "saved")
    if (typeof saved !== "boolean") invalid("saved")
    return { agent, saved }
  })
  const devicesOn = own(item, "devicesOn")
  if (!Array.isArray(devicesOn) || devicesOn.length > MAX_DEVICES) invalid("devicesOn")
  const names: string[] = []
  let last = ""
  for (const name of devicesOn) {
    if (typeof name !== "string" || !peerName.test(name)) invalid("devicesOn")
    if (name <= last) invalid("devicesOn order")
    names.push(name)
    last = name
  }
  return { logins: read, devicesOn: names }
}
