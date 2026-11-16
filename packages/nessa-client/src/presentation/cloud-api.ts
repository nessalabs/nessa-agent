import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  CloudErrorCode,
  type Cloud,
  type CloudChoice,
} from "../generated/product.js"

/** Which cloud new thread VMs are made on. A thread whose VM exists stays on
 * the cloud it was made on. */
export type CloudApi = {
  /** The cloud new thread VMs are made on. */
  read(): Promise<CloudChoice>
  /** Make new thread VMs on `cloud`, which must be connected. The choice is
   * recorded with who made it before it is saved. */
  choose(cloud: Cloud): Promise<CloudChoice>
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
/** Redacted cloud choice failure. */
export class NessaCloudError extends Error {
  /** A recognized gateway refusal, or undefined for an unclassified failure. */
  readonly refusal: CloudErrorCode | undefined
  constructor(cause: unknown) {
    const code = cause instanceof NessaRpcError ? cause.code : undefined
    const refusal = Object.values(CloudErrorCode).find((value) => value === code)
    super(refusal ? `Cloud choice refused (${refusal})` : "Cloud choice unavailable")
    this.name = "NessaCloudError"
    this.refusal = refusal
  }
}
export function createCloudApi(session: RpcRequester): CloudApi {
  const call = async (method: string, params: Record<string, unknown>) => {
    try {
      return cloudChoice(await session.request(method, params))
    } catch (cause) {
      throw new NessaCloudError(cause)
    }
  }
  return {
    read: () => call(ProductMethod.CloudRead, {}),
    choose: (cloud) => call(ProductMethod.CloudChoose, { cloud }),
  }
}
