/**
 * The answer of every `github.*` method, held to the schema's shape before
 * anyone believes it. What the sign-in may do is the gateway's; this only
 * checks that an answer is one the schema describes, and that each part is
 * present exactly in the state that carries it.
 */
import {
  GithubConnectEnd,
  GithubConnectState,
  type GithubAccount,
  type GithubCode,
  type GithubConnection,
} from "../generated/product.js"

function invalid(what: string): never {
  throw new Error(`github response has invalid ${what}`)
}

function record(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) invalid(what)
  return value as Record<string, unknown>
}

function own(item: Record<string, unknown>, key: string): unknown {
  return Object.hasOwn(item, key) ? item[key] : undefined
}

function member<T extends string>(
  table: Record<string, T>,
  value: unknown,
): T | undefined {
  return typeof value === "string" && (Object.values(table) as string[]).includes(value)
    ? (value as T)
    : undefined
}

function text(value: unknown, most: number, what: string): string {
  if (typeof value !== "string" || value.length === 0 || value.length > most)
    invalid(what)
  return value
}

function whole(value: unknown, what: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) invalid(what)
  return value as number
}

const allowed = new Set(["state", "account", "code", "last", "unrecorded", "installUrl"])

function account(value: unknown): GithubAccount {
  const item = record(value, "account")
  return {
    id: whole(own(item, "id"), "account id"),
    login: text(own(item, "login"), 256, "login"),
  }
}

function code(value: unknown): GithubCode {
  const item = record(value, "code")
  return {
    userCode: text(own(item, "userCode"), 64, "userCode"),
    verificationUri: text(own(item, "verificationUri"), 2048, "verificationUri"),
    expiresInSeconds: whole(own(item, "expiresInSeconds"), "expiresInSeconds"),
  }
}

/** The answer to `github.status`, `github.connect`, `github.cancel` and `github.disconnect`. */
export function githubConnection(value: unknown): GithubConnection {
  const item = record(value, "shape")
  if (Object.keys(item).some((key) => !allowed.has(key))) invalid("shape")
  const state = member(GithubConnectState, own(item, "state"))
  if (!state) invalid("state")
  const installUrl = text(own(item, "installUrl"), 2048, "installUrl")
  const present = (key: string) => own(item, key) !== undefined
  if (present("account") !== (state === GithubConnectState.Connected)) invalid("account")
  if (present("code") !== (state === GithubConnectState.Connecting)) invalid("code")
  if (present("last") && state !== GithubConnectState.SignedOut) invalid("last")
  const last = present("last") ? member(GithubConnectEnd, own(item, "last")) : undefined
  if (present("last") && !last) invalid("last")
  // Only ever `true`, and never while a code is showing.
  if (
    present("unrecorded") &&
    (own(item, "unrecorded") !== true || state === GithubConnectState.Connecting)
  )
    invalid("unrecorded")
  return {
    state,
    installUrl,
    ...(present("account") ? { account: account(own(item, "account")) } : {}),
    ...(present("code") ? { code: code(own(item, "code")) } : {}),
    ...(last ? { last } : {}),
    ...(present("unrecorded") ? { unrecorded: true as const } : {}),
  }
}
