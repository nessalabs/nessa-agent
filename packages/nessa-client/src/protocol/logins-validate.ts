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
  const item = record(value, ["state", "attempt", "authorizeUrl", "accountId", "last"])
  const state = own(item, "state")
  if (state === "idle") {
    if (Object.hasOwn(item, "authorizeUrl") || Object.hasOwn(item, "accountId"))
      invalid("chatgpt sign-in")
    // An end names the attempt it was; no end, no attempt.
    if (Object.hasOwn(item, "last") !== Object.hasOwn(item, "attempt"))
      invalid("chatgpt sign-in attempt")
    if (!Object.hasOwn(item, "last")) return { state }
    const last = own(item, "last")
    if (typeof last !== "string" || !ends.has(last)) invalid("chatgpt sign-in end")
    return {
      state,
      attempt: accountId(own(item, "attempt"), "chatgpt sign-in attempt"),
      last: last as ChatgptSignInEnd,
    }
  }
  if (state !== "waiting" || Object.hasOwn(item, "last")) invalid("chatgpt sign-in")
  const attempt = accountId(own(item, "attempt"), "chatgpt sign-in attempt")
  const authorizeUrl = own(item, "authorizeUrl")
  if (
    typeof authorizeUrl !== "string" ||
    authorizeUrl.length > bounds.maxChatgptAuthorizeUrlBytes ||
    !/^https?:\/\//.test(authorizeUrl)
  )
    invalid("chatgpt authorizeUrl")
  if (!Object.hasOwn(item, "accountId")) return { state, attempt, authorizeUrl }
  return {
    state,
    attempt,
    authorizeUrl,
    accountId: accountId(own(item, "accountId"), "chatgpt sign-in account"),
  }
}

function chatgptLogins(value: unknown): ChatgptLogins {
  const item = record(value, ["accounts", "signIn", "started"])
  const accounts = own(item, "accounts")
  if (!Array.isArray(accounts) || accounts.length > bounds.maxChatgptAccounts)
    invalid("chatgpt accounts")
  const read = accounts.map(chatgptAccount)
  if (new Set(read.map((account) => account.id)).size !== read.length)
    invalid("chatgpt account ids")
  if (read.filter((account) => account.inUse).length > 1) invalid("chatgpt inUse")
  const signIn = chatgptSignIn(own(item, "signIn"))
  if (!Object.hasOwn(item, "started")) return { accounts: read, signIn }
  return {
    accounts: read,
    signIn,
    started: accountId(own(item, "started"), "chatgpt sign-in started"),
  }
}

export function loginsStatus(value: unknown): LoginsStatus {
  const item = record(value, ["logins", "chatgpt"])
  const logins = own(item, "logins")
  if (!Array.isArray(logins) || logins.length !== agents.length) invalid("logins")
  const read: SavedLogin[] = agents.map((agent, index) => {
    const login = record(logins[index], ["agent", "saved"])
    if (own(login, "agent") !== agent) invalid("agent")
    const saved = own(login, "saved")
    if (typeof saved !== "boolean") invalid("saved")
    return { agent, saved }
  })
  if (!Object.hasOwn(item, "chatgpt")) return { logins: read }
  return { logins: read, chatgpt: chatgptLogins(own(item, "chatgpt")) }
}
