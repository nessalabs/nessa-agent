import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  BoatErrorCode,
  type BoatSaveParams,
  type BoatSignInState,
  type BoatStatus,
} from "../generated/product.js"

/** The saved infrastructure connection. Status never contains a token. */
export type BoatApi = {
  /** Read saved configuration. Connected attests account validation at save time,
   * not a live model turn. A locked vault prevents reading its entry. */
  status(): Promise<BoatStatus>
  /** Validate and encrypt a user-owned Boat connection. Keep the token transient.
   * Calls are never replayed; after an uncertain outcome, read status. */
  save(params: BoatSaveParams): Promise<BoatStatus>
  /** Begin Sign in with Boat, ending any earlier one. The answer's
   * `signIn.url` is the Boat page to open in the person's browser; the
   * gateway waits on this machine for Boat to send the browser back, then
   * saves the connection. Read status to follow it. */
  signIn(): Promise<BoatStatus>
  /** Stop waiting for a sign-in. Nothing is saved. */
  cancelSignIn(): Promise<BoatStatus>
  /** Forget this connection. Existing remote credentials/VMs are not revoked or
   * deleted by this command. Read status after an uncertain response. */
  disconnect(): Promise<BoatStatus>
}
const STATUS_KEYS = new Set([
  "state",
  "environment",
  "image",
  "via",
  "login",
  "signInAvailable",
  "signIn",
])
/** Read a status: only the fields the schema names, each in the shape it
 * names, and connection facts only while connected. */
export function boatStatus(value: unknown): BoatStatus {
  const invalid = () => new TypeError("Invalid Boat status")
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw invalid()
  const item = value as Record<string, unknown>
  const own = (name: string) => (Object.hasOwn(item, name) ? item[name] : undefined)
  if (Object.keys(item).some((key) => !STATUS_KEYS.has(key))) throw invalid()
  const state = own("state")
  const text = (field: unknown) => typeof field === "string" && field.length > 0
  const status: BoatStatus = { state: "disconnected" }
  if (state === "locked" || state === "disconnected") {
    status.state = state
    for (const name of ["environment", "image", "via", "login"])
      if (Object.hasOwn(item, name)) throw invalid()
  } else if (state === "connected") {
    const environment = own("environment")
    const image = own("image")
    const via = own("via")
    const login = own("login")
    if (!text(environment) || !text(image)) throw invalid()
    status.state = state
    status.environment = environment as string
    status.image = image as string
    if (via !== undefined) {
      if (via !== "signIn" && via !== "token") throw invalid()
      status.via = via
    }
    if (login !== undefined) {
      if (!text(login) || via !== "signIn") throw invalid()
      status.login = login as string
    }
  } else throw invalid()
  const available = own("signInAvailable")
  if (available !== undefined) {
    if (typeof available !== "boolean") throw invalid()
    status.signInAvailable = available
  }
  const signIn = own("signIn")
  if (signIn !== undefined) status.signIn = boatSignIn(signIn)
  return status
}
function boatSignIn(value: unknown): BoatSignInState {
  const invalid = () => new TypeError("Invalid Boat status")
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw invalid()
  const item = value as Record<string, unknown>
  const own = (name: string) => (Object.hasOwn(item, name) ? item[name] : undefined)
  const state = own("state")
  const url = own("url")
  const cause = own("cause")
  if (state === "waiting" && Object.keys(item).length === 2 && typeof url === "string") {
    const parsed = URL.canParse(url) ? new URL(url) : null
    if (parsed && (parsed.protocol === "https:" || parsed.protocol === "http:"))
      return { state, url }
  } else if (
    state === "failed" &&
    Object.keys(item).length === 2 &&
    typeof cause === "string" &&
    cause.length > 0 &&
    cause.length <= 64
  )
    return { state, cause }
  throw invalid()
}
/** Redacted Boat setup failure. Provider diagnostics and secret parameters are discarded. */
export class NessaBoatError extends Error {
  /** A recognized gateway refusal, or undefined for an unclassified failure. */
  readonly refusal: BoatErrorCode | undefined
  constructor(cause: unknown) {
    const code = cause instanceof NessaRpcError ? cause.code : undefined
    const refusal = Object.values(BoatErrorCode).find((value) => value === code)
    super(refusal ? `Boat operation refused (${refusal})` : "Boat unavailable")
    this.name = "NessaBoatError"
    this.refusal = refusal
  }
}
export function createBoatApi(session: RpcRequester): BoatApi {
  const call = async <T>(
    method: string,
    params: Record<string, unknown>,
    decode: (value: unknown) => T,
  ): Promise<T> => {
    try {
      return decode(await session.request(method, params))
    } catch (cause) {
      throw new NessaBoatError(cause)
    }
  }
  return {
    status: () => call(ProductMethod.BoatStatus, {}, boatStatus),
    save: (params) => call(ProductMethod.BoatSave, { ...params }, boatStatus),
    signIn: () => call(ProductMethod.BoatSignIn, {}, boatStatus),
    cancelSignIn: () => call(ProductMethod.BoatCancelSignIn, {}, boatStatus),
    disconnect: () => call(ProductMethod.BoatDisconnect, {}, boatStatus),
  }
}
