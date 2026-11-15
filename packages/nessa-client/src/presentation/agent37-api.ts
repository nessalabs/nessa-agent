import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  Agent37ErrorCode,
  type Agent37SaveParams,
  type Agent37Status,
  type Cloud,
  type CloudChoice,
} from "../generated/product.js"

/** The saved Agent37 workspace, and which cloud new thread VMs are made on.
 * Status never contains a key. */
export type Agent37Api = {
  /** Read the saved workspace. A locked vault prevents reading its entry. */
  status(): Promise<Agent37Status>
  /** Validate and encrypt a user-owned Agent37 workspace. Keep the key
   * transient. Calls are never replayed; after an uncertain outcome, read
   * status. */
  save(params: Agent37SaveParams): Promise<Agent37Status>
  /** Forget the workspace. Its VMs are not deleted. If new thread VMs were
   * going to Agent37, they go to Boat again. */
  disconnect(): Promise<Agent37Status>
  /** The cloud new thread VMs are made on. */
  cloud(): Promise<CloudChoice>
  /** Make new thread VMs on `cloud`, which must be connected. A thread whose
   * VM exists stays on the cloud it was made on. */
  chooseCloud(cloud: Cloud): Promise<CloudChoice>
}
export function agent37Status(value: unknown): Agent37Status {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new TypeError("Invalid Agent37 status")
  const item = value as Record<string, unknown>
  const own = (name: string) => (Object.hasOwn(item, name) ? item[name] : undefined)
  const state = own("state")
  const template = own("template")
  if (state === "locked" || state === "disconnected") {
    if (Object.keys(item).length === 1) return { state }
  } else if (
    state === "connected" &&
    Object.keys(item).length === 2 &&
    typeof template === "string" &&
    template.length > 0
  )
    return { state, template }
  throw new TypeError("Invalid Agent37 status")
}
export function cloudChoice(value: unknown): CloudChoice {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new TypeError("Invalid cloud choice")
  const item = value as Record<string, unknown>
  const cloud = Object.hasOwn(item, "cloud") ? item.cloud : undefined
  if (Object.keys(item).length === 1 && (cloud === "boat" || cloud === "agent37"))
    return { cloud }
  throw new TypeError("Invalid cloud choice")
}
/** Redacted Agent37 setup failure. Provider diagnostics and secret parameters are discarded. */
export class NessaAgent37Error extends Error {
  /** A recognized gateway refusal, or undefined for an unclassified failure. */
  readonly refusal: Agent37ErrorCode | undefined
  constructor(cause: unknown) {
    const code = cause instanceof NessaRpcError ? cause.code : undefined
    const refusal = Object.values(Agent37ErrorCode).find((value) => value === code)
    super(refusal ? `Agent37 operation refused (${refusal})` : "Agent37 unavailable")
    this.name = "NessaAgent37Error"
    this.refusal = refusal
  }
}
export function createAgent37Api(session: RpcRequester): Agent37Api {
  const call = async <T>(
    method: string,
    params: Record<string, unknown>,
    decode: (value: unknown) => T,
  ): Promise<T> => {
    try {
      return decode(await session.request(method, params))
    } catch (cause) {
      throw new NessaAgent37Error(cause)
    }
  }
  return {
    status: () => call(ProductMethod.Agent37Status, {}, agent37Status),
    save: (params) => call(ProductMethod.Agent37Save, { key: params.key }, agent37Status),
    disconnect: () => call(ProductMethod.Agent37Disconnect, {}, agent37Status),
    cloud: () => call(ProductMethod.CloudRead, {}, cloudChoice),
    chooseCloud: (cloud) => call(ProductMethod.CloudChoose, { cloud }, cloudChoice),
  }
}
