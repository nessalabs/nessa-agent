/**
 * The answer of every `logins.*` method, held to the schema's shape before
 * anyone believes it.
 */
import {
  ChatgptSignInEnd,
  LoginAgent,
  bounds,
  type ChatgptAccount,
  type ChatgptLogins,
  type ChatgptSignIn,
  type LoginsStatus,
  type SavedLogin,
} from "../generated/product.js"

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
const peerName = new RegExp(bounds.loginDevicePattern)

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const ends = new Set<string>(Object.values(ChatgptSignInEnd))
const encoder = new TextEncoder()

function accountId(value: unknown, what: string): string {
  if (typeof value !== "string" || !uuid.test(value)) invalid(what)
  return value
}

function chatgptAccount(value: unknown): ChatgptAccount {
  const item = record(value, ["id", "email", "inUse", "needsSignIn"])
  const email = own(item, "email")
  if (
    typeof email !== "string" ||
    encoder.encode(email).length > bounds.maxChatgptEmailBytes
  )
    invalid("chatgpt email")
  const inUse = own(item, "inUse")
  const needsSignIn = own(item, "needsSignIn")
  if (typeof inUse !== "boolean" || typeof needsSignIn !== "boolean")
    invalid("chatgpt account")
  return {
    id: accountId(own(item, "id"), "chatgpt account id"),
    email,
    inUse,
    needsSignIn,
  }
}

function chatgptSignIn(value: unknown): ChatgptSignIn {
  const item = record(value, ["state", "authorizeUrl", "accountId", "last"])
  const state = own(item, "state")
  if (state === "idle") {
    if (Object.hasOwn(item, "authorizeUrl") || Object.hasOwn(item, "accountId"))
      invalid("chatgpt sign-in")
    if (!Object.hasOwn(item, "last")) return { state }
    const last = own(item, "last")
    if (typeof last !== "string" || !ends.has(last)) invalid("chatgpt sign-in end")
    return { state, last: last as ChatgptSignInEnd }
  }
  if (state !== "waiting" || Object.hasOwn(item, "last")) invalid("chatgpt sign-in")
  const authorizeUrl = own(item, "authorizeUrl")
  if (
    typeof authorizeUrl !== "string" ||
    authorizeUrl.length > bounds.maxChatgptAuthorizeUrlBytes ||
    !/^https?:\/\//.test(authorizeUrl)
  )
    invalid("chatgpt authorizeUrl")
  if (!Object.hasOwn(item, "accountId")) return { state, authorizeUrl }
  return {
    state,
    authorizeUrl,
    accountId: accountId(own(item, "accountId"), "chatgpt sign-in account"),
  }
}

function chatgptLogins(value: unknown): ChatgptLogins {
  const item = record(value, ["accounts", "signIn"])
  const accounts = own(item, "accounts")
  if (!Array.isArray(accounts) || accounts.length > bounds.maxChatgptAccounts)
    invalid("chatgpt accounts")
  const read = accounts.map(chatgptAccount)
  if (new Set(read.map((account) => account.id)).size !== read.length)
    invalid("chatgpt account ids")
  if (read.filter((account) => account.inUse).length > 1) invalid("chatgpt inUse")
  return { accounts: read, signIn: chatgptSignIn(own(item, "signIn")) }
}

export function loginsStatus(value: unknown): LoginsStatus {
  const item = record(value, ["logins", "devicesOn", "chatgpt"])
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
  if (!Array.isArray(devicesOn) || devicesOn.length > bounds.maxLoginDevicesOn)
    invalid("devicesOn")
  const names: string[] = []
  let last = ""
  for (const name of devicesOn) {
    if (typeof name !== "string" || !peerName.test(name)) invalid("devicesOn")
    if (name <= last) invalid("devicesOn order")
    names.push(name)
    last = name
  }
  if (!Object.hasOwn(item, "chatgpt")) return { logins: read, devicesOn: names }
  return { logins: read, devicesOn: names, chatgpt: chatgptLogins(own(item, "chatgpt")) }
}
