import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  CloudErrorCode,
  CloudBoxErrorCode,
  CloudBoxState,
  type Cloud,
  type CloudChoice,
  type CloudBoxSummary,
  type CloudBoxCreateParams,
  type CloudBoxCreateResult,
  type CloudBoxWakeResult,
  type CloudBoxDeleteResult,
} from "../generated/product.js"

/** Persistent cloud gateway VM lifecycle operations. */
export type CloudBoxesApi = {
  /** List all active (running or sleeping) persistent cloud gateway VMs. */
  list(): Promise<readonly CloudBoxSummary[]>
  /** Provision and bootstrap a new persistent cloud gateway VM. Returns the
   * box summary and its one-time enrollment credential. */
  create(params: CloudBoxCreateParams): Promise<CloudBoxCreateResult>
  /** Wake a sleeping cloud gateway VM and return its verified endpoint URL. */
  wake(boxId: string): Promise<CloudBoxWakeResult>
  /** Stop an idle cloud gateway VM without losing its persistent disk. */
  sleep(boxId: string): Promise<CloudBoxSummary>
  /** Rename a persistent cloud gateway VM. */
  rename(boxId: string, name: string): Promise<CloudBoxSummary>
  /** Permanently destroy a persistent cloud gateway VM and its disk. */
  delete(boxId: string): Promise<CloudBoxDeleteResult>
}

/** Which cloud new thread VMs are made on, plus persistent cloud gateway VM
 * orchestration (`boxes`). */
export type CloudApi = {
  /** The cloud new thread VMs are made on. */
  read(): Promise<CloudChoice>
  /** Make new thread VMs on `cloud`, which must be connected. The choice is
   * recorded with who made it before it is saved. */
  choose(cloud: Cloud): Promise<CloudChoice>
  /** Manage persistent cloud gateway VMs (`cloud.boxes.*`). */
  readonly boxes: CloudBoxesApi
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

const CLOUD_BOX_STATES = new Set<string>(Object.values(CloudBoxState))

function readOwnString(obj: Record<string, unknown>, key: string): string {
  const value = Object.hasOwn(obj, key) ? obj[key] : undefined
  if (typeof value !== "string" || value.length === 0) {
    throw new TypeError(`Invalid cloud box field: ${key}`)
  }
  return value
}

function readOwnNumber(obj: Record<string, unknown>, key: string): number {
  const value = Object.hasOwn(obj, key) ? obj[key] : undefined
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    throw new TypeError(`Invalid cloud box field: ${key}`)
  }
  return value
}

function parseCloudBoxSummary(value: unknown): CloudBoxSummary {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError("Invalid cloud box summary")
  }
  const obj = value as Record<string, unknown>
  const cloud = readOwnString(obj, "cloud")
  if (cloud !== "boat" && cloud !== "agent37") {
    throw new TypeError("Invalid cloud box provider")
  }
  const state = readOwnString(obj, "state")
  if (!CLOUD_BOX_STATES.has(state)) {
    throw new TypeError("Invalid cloud box state")
  }
  return {
    boxId: readOwnString(obj, "boxId"),
    name: readOwnString(obj, "name"),
    cloud,
    vm: readOwnString(obj, "vm"),
    gatewayId: readOwnString(obj, "gatewayId"),
    url: readOwnString(obj, "url"),
    state: state as CloudBoxState,
    createdAtMs: readOwnNumber(obj, "createdAtMs"),
    updatedAtMs: readOwnNumber(obj, "updatedAtMs"),
  }
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

/** Redacted persistent cloud gateway VM lifecycle failure. */
export class NessaCloudBoxError extends Error {
  /** A recognized gateway refusal, or undefined for an unclassified failure. */
  readonly refusal: CloudBoxErrorCode | undefined
  constructor(cause: unknown) {
    const code = cause instanceof NessaRpcError ? cause.code : undefined
    const refusal = Object.values(CloudBoxErrorCode).find((value) => value === code)
    super(refusal ? `Cloud box refused (${refusal})` : "Cloud box operation unavailable")
    this.name = "NessaCloudBoxError"
    this.refusal = refusal
  }
}

/** Minimum RPC deadline for creating and bootstrapping a persistent Cloud Box VM. */
export const CLOUD_BOX_CREATE_TIMEOUT_MS = 240_000
/** Minimum RPC deadline for waking a sleeping persistent Cloud Box VM. */
export const CLOUD_BOX_WAKE_TIMEOUT_MS = 150_000
/** Minimum RPC deadline for sleeping or deleting a persistent Cloud Box VM. */
export const CLOUD_BOX_MUTATION_TIMEOUT_MS = 60_000

export function createCloudApi(session: RpcRequester): CloudApi {
  const callChoice = async (method: string, params: Record<string, unknown>) => {
    try {
      return cloudChoice(await session.request(method, params))
    } catch (cause) {
      throw new NessaCloudError(cause)
    }
  }
  const callBox = async <T>(
    method: string,
    params: Record<string, unknown>,
    parse: (raw: unknown) => T,
    atLeastMs?: number,
  ): Promise<T> => {
    try {
      const raw =
        atLeastMs !== undefined
          ? await session.request(method, params, { atLeastMs })
          : await session.request(method, params)
      return parse(raw)
    } catch (cause) {
      throw new NessaCloudBoxError(cause)
    }
  }

  const boxes: CloudBoxesApi = {
    list: () =>
      callBox(ProductMethod.CloudBoxesList, {}, (raw) => {
        if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
          throw new TypeError("Invalid cloud boxes list")
        }
        const obj = raw as Record<string, unknown>
        const items = Object.hasOwn(obj, "boxes") ? obj.boxes : undefined
        if (!Array.isArray(items)) {
          throw new TypeError("Invalid cloud boxes list")
        }
        return items.map(parseCloudBoxSummary)
      }),
    create: (params) =>
      callBox(
        ProductMethod.CloudBoxesCreate,
        { ...params },
        (raw) => {
          if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
            throw new TypeError("Invalid cloud box create result")
          }
          const obj = raw as Record<string, unknown>
          const cloudBox = parseCloudBoxSummary(
            Object.hasOwn(obj, "cloudBox") ? obj.cloudBox : null,
          )
          const credential = readOwnString(obj, "credential")
          return { cloudBox, credential }
        },
        CLOUD_BOX_CREATE_TIMEOUT_MS,
      ),
    wake: (boxId) =>
      callBox(
        ProductMethod.CloudBoxesWake,
        { boxId },
        (raw) => {
          if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
            throw new TypeError("Invalid cloud box wake result")
          }
          const obj = raw as Record<string, unknown>
          return {
            boxId: readOwnString(obj, "boxId"),
            gatewayId: readOwnString(obj, "gatewayId"),
            url: readOwnString(obj, "url"),
          }
        },
        CLOUD_BOX_WAKE_TIMEOUT_MS,
      ),
    sleep: (boxId) =>
      callBox(
        ProductMethod.CloudBoxesSleep,
        { boxId },
        (raw) => {
          if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
            throw new TypeError("Invalid cloud box sleep result")
          }
          const obj = raw as Record<string, unknown>
          return parseCloudBoxSummary(
            Object.hasOwn(obj, "cloudBox") ? obj.cloudBox : null,
          )
        },
        CLOUD_BOX_MUTATION_TIMEOUT_MS,
      ),
    rename: (boxId, name) =>
      callBox(ProductMethod.CloudBoxesRename, { boxId, name }, (raw) => {
        if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
          throw new TypeError("Invalid cloud box rename result")
        }
        const obj = raw as Record<string, unknown>
        return parseCloudBoxSummary(Object.hasOwn(obj, "cloudBox") ? obj.cloudBox : null)
      }),
    delete: (boxId) =>
      callBox(
        ProductMethod.CloudBoxesDelete,
        { boxId },
        (raw) => {
          if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
            throw new TypeError("Invalid cloud box delete result")
          }
          const obj = raw as Record<string, unknown>
          const deleted = Object.hasOwn(obj, "deleted") ? obj.deleted : undefined
          if (deleted !== true) {
            throw new TypeError("Invalid cloud box delete confirmation")
          }
          return {
            boxId: readOwnString(obj, "boxId"),
            deleted: true,
          }
        },
        CLOUD_BOX_MUTATION_TIMEOUT_MS,
      ),
  }

  return {
    read: () => callChoice(ProductMethod.CloudRead, {}),
    choose: (cloud) => callChoice(ProductMethod.CloudChoose, { cloud }),
    boxes,
  }
}
