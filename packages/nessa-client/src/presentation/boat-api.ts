import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  BoatErrorCode,
  vmLifetimeSeconds,
  type BoatRuntimeReadParams,
  type BoatRuntimeSetParams,
  type BoatRuntimeSettings,
  type BoatVmLifetime,
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
  /** Read ordinary configuration, without reading or unlocking the token vault.
   * Omitted channel selects the global default; override absence means inheritance. */
  runtimeSettings(params?: BoatRuntimeReadParams): Promise<BoatRuntimeSettings>
  /** Save the default or an authorized channel override in gateway config.
   * Applies on VM create/resume; it does not change active deadlines. No automatic
   * replay: read settings after an uncertain outcome. */
  setRuntimeSettings(params: BoatRuntimeSetParams): Promise<BoatRuntimeSettings>
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
function lifetime(value: unknown): BoatVmLifetime {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new TypeError("Invalid VM lifetime")
  const item = value as Record<string, unknown>
  const mode = Object.hasOwn(item, "mode") ? item.mode : undefined
  const seconds = Object.hasOwn(item, "seconds") ? item.seconds : undefined
  if (mode === "never" && Object.keys(item).length === 1) return { mode }
  if (
    mode === "after" &&
    Object.keys(item).length === 2 &&
    typeof seconds === "number" &&
    Number.isSafeInteger(seconds) &&
    seconds >= vmLifetimeSeconds.minimum &&
    seconds <= vmLifetimeSeconds.maximum
  )
    return { mode, seconds }
  throw new TypeError("Invalid VM lifetime")
}
export function boatRuntimeSettings(value: unknown): BoatRuntimeSettings {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new TypeError("Invalid Boat runtime settings")
  const item = value as Record<string, unknown>
  if (
    !Object.hasOwn(item, "defaultLifetime") ||
    Object.keys(item).some(
      (key) =>
        key !== "defaultLifetime" && key !== "channelOverride" && key !== "channelId",
    )
  )
    throw new TypeError("Invalid Boat runtime settings")
  const defaultLifetime = lifetime(item.defaultLifetime)
  let channelId: string | undefined
  if (Object.hasOwn(item, "channelId")) {
    const id = item.channelId
    if (typeof id !== "string" || id.length === 0)
      throw new TypeError("Invalid runtime settings scope")
    channelId = id
  }
  if (Object.hasOwn(item, "channelOverride") && channelId === undefined)
    throw new TypeError("Unscoped channel override")
  return {
    defaultLifetime,
    ...(channelId === undefined ? {} : { channelId }),
    ...(Object.hasOwn(item, "channelOverride")
      ? { channelOverride: lifetime(item.channelOverride) }
      : {}),
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
  const runtimeCall = async (
    method: string,
    params: BoatRuntimeReadParams | BoatRuntimeSetParams,
  ) => {
    let channelId: string | undefined
    let request: Record<string, unknown>
    try {
      const scoped = Object.hasOwn(params, "channelId")
      if (!scoped && "channelId" in params) throw new TypeError("Inherited runtime scope")
      // Capture scope once, including non-enumerable own properties. Spreading
      // the original afterward could omit it or invoke its getter again and
      // dispatch a different scope before response binding can reject it.
      channelId = scoped ? params.channelId : undefined
      const fields = params as Record<string, unknown>
      const rest = Object.fromEntries(
        Object.keys(fields).flatMap((key) =>
          key !== "channelId" && Object.hasOwn(fields, key) ? [[key, fields[key]]] : [],
        ),
      )
      request = scoped ? { ...rest, channelId } : rest
    } catch (cause) {
      throw new NessaBoatError(cause)
    }
    return call(method, request, (value) => {
      const settings = boatRuntimeSettings(value)
      if (settings.channelId !== channelId)
        throw new TypeError("Runtime settings scope mismatch")
      return settings
    })
  }
  return {
    status: () => call(ProductMethod.BoatStatus, {}, boatStatus),
    save: (params) => call(ProductMethod.BoatSave, { ...params }, boatStatus),
    disconnect: () => call(ProductMethod.BoatDisconnect, {}, boatStatus),
    runtimeSettings: (params = {}) =>
      runtimeCall(ProductMethod.BoatRuntimeSettings, params),
    setRuntimeSettings: (params) =>
      runtimeCall(ProductMethod.BoatSetRuntimeSettings, params),
  }
}
