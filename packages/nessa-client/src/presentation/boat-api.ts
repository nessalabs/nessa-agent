import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  BoatErrorCode,
  type BoatSaveParams,
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
  /** Forget this connection. Existing remote credentials/VMs are not revoked or
   * deleted by this command. Read status after an uncertain response. */
  disconnect(): Promise<BoatStatus>
}
export function boatStatus(value: unknown): BoatStatus {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new TypeError("Invalid Boat status")
  const item = value as Record<string, unknown>
  const own = (name: string) => (Object.hasOwn(item, name) ? item[name] : undefined)
  const state = own("state")
  const environment = own("environment")
  const image = own("image")
  if (state === "locked" || state === "disconnected") {
    if (Object.keys(item).length === 1) return { state }
  } else if (
    state === "connected" &&
    Object.keys(item).length === 3 &&
    typeof environment === "string" &&
    environment.length > 0 &&
    typeof image === "string" &&
    image.length > 0
  )
    return { state, environment, image }
  throw new TypeError("Invalid Boat status")
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
  const call = async (method: string, params: Record<string, unknown> = {}) => {
    try {
      return boatStatus(await session.request(method, params))
    } catch (cause) {
      throw new NessaBoatError(cause)
    }
  }
  return {
    status: () => call(ProductMethod.BoatStatus),
    save: (params) => call(ProductMethod.BoatSave, { ...params }),
    disconnect: () => call(ProductMethod.BoatDisconnect),
  }
}
