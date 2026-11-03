import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  type DeviceDisk,
  type DeviceMachine,
  type DeviceWorker,
  type DevicesListResult,
  type DevicesPool,
  type PoolPreference,
} from "../generated/product.js"

/** The hub's devices: the machine the gateway runs on and its workers. */
export type DevicesApi = {
  /**
   * The gateway's own machine (on the first page only, absent when it
   * cannot be read) and one page of the paired environments that work for
   * it, connected now or seen before: at most 64, in the order of their
   * keys, each after `after`.
   * `next` is the next page's `after`, absent on the last page. Figures are
   * a snapshot; read again to follow them.
   */
  list(after?: string): Promise<DevicesListResult>
  /**
   * Where new `peer:any` threads go: the Nessa default, `channelId`'s own
   * preference when it has one, and when a computer counts as busy.
   */
  pool(channelId?: string): Promise<DevicesPool>
  /**
   * Save the Nessa default (no `channelId`) or a channel's own preference;
   * `undefined` empties the default, or makes the channel follow it again.
   */
  setPool(
    preference: PoolPreference | undefined,
    channelId?: string,
  ): Promise<DevicesPool>
}

export function createDevicesApi(session: RpcRequester): DevicesApi {
  return {
    list: async (after) =>
      devicesList(
        await session.request(
          ProductMethod.DevicesList,
          after === undefined ? {} : { after },
        ),
      ),
    pool: async (channelId) =>
      devicesPool(
        await session.request(
          ProductMethod.DevicesPool,
          channelId === undefined ? {} : { channelId },
        ),
      ),
    setPool: async (preference, channelId) =>
      devicesPool(
        await session.request(ProductMethod.DevicesSetPool, {
          ...(channelId === undefined ? {} : { channelId }),
          ...(preference === undefined
            ? { mode: "default" }
            : { mode: "custom", preference }),
        }),
      ),
  }
}

const MAX_POOL_ENTRIES = 64
const MAX_CHANNEL_ID = 128

/** Reads a `devices.pool` or `devices.setPool` answer, refusing anything off its schema. */
export function devicesPool(value: unknown): DevicesPool {
  const invalid = () => new TypeError("Invalid pool preferences")
  if (
    !isRecord(value) ||
    !onlyKeys(
      value,
      ["defaultPreference", "busyCpuPercent", "busyMemoryPercent"],
      ["channelId", "channelOverride"],
    ) ||
    (Object.hasOwn(value, "channelId") &&
      (typeof value.channelId !== "string" ||
        value.channelId.length === 0 ||
        value.channelId.length > MAX_CHANNEL_ID)) ||
    !percent(value.busyCpuPercent) ||
    !percent(value.busyMemoryPercent)
  )
    throw invalid()
  return {
    ...(Object.hasOwn(value, "channelId")
      ? { channelId: value.channelId as string }
      : {}),
    defaultPreference: preferenceOf(value.defaultPreference),
    ...(Object.hasOwn(value, "channelOverride")
      ? { channelOverride: preferenceOf(value.channelOverride) }
      : {}),
    busyCpuPercent: value.busyCpuPercent as number,
    busyMemoryPercent: value.busyMemoryPercent as number,
  }
}

function preferenceOf(value: unknown): PoolPreference {
  const names = (list: unknown): list is string[] =>
    Array.isArray(list) &&
    list.length <= MAX_POOL_ENTRIES &&
    list.every((name) => typeof name === "string" && ENVIRONMENT.test(name))
  if (
    !isRecord(value) ||
    !onlyKeys(value, ["order", "excluded"], []) ||
    !names(value.order) ||
    !names(value.excluded) ||
    new Set([...value.order, ...value.excluded]).size !==
      value.order.length + value.excluded.length
  )
    throw new TypeError("Invalid pool preferences")
  return { order: [...value.order], excluded: [...value.excluded] }
}

function percent(value: unknown): boolean {
  return count(value, 100) && (value as number) >= 1
}

const MACHINE_KEYS = [
  "hostname",
  "os",
  "osVersion",
  "arch",
  "cpus",
  "cpuPermille",
  "memoryTotalBytes",
  "memoryUsedBytes",
  "diskTotalBytes",
  "diskUsedBytes",
] as const
const MAX_TEXT = 128
const MAX_DISKS = 16
const MAX_WORKERS = 64
const MAX_LISTENING = 64
const ENVIRONMENT = /^peer:[0-9a-f]{64}$/
const STATES = new Set<string>(["online", "draining", "offline"])

/** Reads a `devices.list` answer, refusing anything off its schema. */
export function devicesList(value: unknown): DevicesListResult {
  const invalid = () => new TypeError("Invalid devices list")
  if (!isRecord(value) || !onlyKeys(value, ["workers"], ["hub", "listening", "next"]))
    throw invalid()
  if (
    Object.hasOwn(value, "next") &&
    (typeof value.next !== "string" || !ENVIRONMENT.test(value.next))
  )
    throw invalid()
  if (
    Object.hasOwn(value, "listening") &&
    (typeof value.listening !== "string" || value.listening.length > MAX_LISTENING)
  )
    throw invalid()
  const workers = value.workers
  if (!Array.isArray(workers) || workers.length > MAX_WORKERS) throw invalid()
  return {
    ...(Object.hasOwn(value, "hub") ? { hub: machine(value.hub) } : {}),
    ...(Object.hasOwn(value, "listening")
      ? { listening: value.listening as string }
      : {}),
    workers: workers.map(worker),
    ...(Object.hasOwn(value, "next") ? { next: value.next as string } : {}),
  }
}

function worker(value: unknown): DeviceWorker {
  if (
    !isRecord(value) ||
    !onlyKeys(value, ["environment", "state", "agents"], ["machine", "lastSeenMs"]) ||
    typeof value.environment !== "string" ||
    !ENVIRONMENT.test(value.environment) ||
    typeof value.state !== "string" ||
    !STATES.has(value.state) ||
    !count(value.agents, 0xffff_ffff) ||
    (Object.hasOwn(value, "lastSeenMs") && !count(value.lastSeenMs))
  )
    throw new TypeError("Invalid devices list")
  return {
    environment: value.environment,
    state: value.state as DeviceWorker["state"],
    agents: value.agents as number,
    ...(Object.hasOwn(value, "machine") ? { machine: machine(value.machine) } : {}),
    ...(Object.hasOwn(value, "lastSeenMs")
      ? { lastSeenMs: value.lastSeenMs as number }
      : {}),
  }
}

function machine(value: unknown): DeviceMachine {
  if (!isRecord(value) || !onlyKeys(value, MACHINE_KEYS, ["disks"]))
    throw new TypeError("Invalid devices list")
  for (const key of ["hostname", "os", "osVersion", "arch"] as const) {
    const text = value[key]
    if (typeof text !== "string" || new TextEncoder().encode(text).length > MAX_TEXT)
      throw new TypeError("Invalid devices list")
  }
  if (
    !count(value.cpus, 0xffff_ffff) ||
    !count(value.cpuPermille, 1000) ||
    !count(value.memoryTotalBytes) ||
    !count(value.memoryUsedBytes) ||
    !count(value.diskTotalBytes) ||
    !count(value.diskUsedBytes) ||
    (value.memoryUsedBytes as number) > (value.memoryTotalBytes as number) ||
    (value.diskUsedBytes as number) > (value.diskTotalBytes as number)
  )
    throw new TypeError("Invalid devices list")
  const disks = value.disks
  if (disks !== undefined && (!Array.isArray(disks) || disks.length > MAX_DISKS))
    throw new TypeError("Invalid devices list")
  return {
    ...(Object.fromEntries(
      MACHINE_KEYS.map((key) => [key, value[key]]),
    ) as unknown as DeviceMachine),
    ...(Array.isArray(disks) ? { disks: disks.map(disk) } : {}),
  }
}

function disk(value: unknown): DeviceDisk {
  if (
    !isRecord(value) ||
    !onlyKeys(value, ["mount", "totalBytes", "usedBytes"], []) ||
    typeof value.mount !== "string" ||
    new TextEncoder().encode(value.mount).length > MAX_TEXT ||
    !count(value.totalBytes) ||
    !count(value.usedBytes) ||
    (value.usedBytes as number) > (value.totalBytes as number)
  )
    throw new TypeError("Invalid devices list")
  return {
    mount: value.mount,
    totalBytes: value.totalBytes as number,
    usedBytes: value.usedBytes as number,
  }
}

function count(value: unknown, maximum = Number.MAX_SAFE_INTEGER): boolean {
  return (
    Number.isSafeInteger(value) && (value as number) >= 0 && (value as number) <= maximum
  )
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
}

function onlyKeys(
  value: Record<string, unknown>,
  required: readonly string[],
  optional: readonly string[],
): boolean {
  return (
    Object.keys(value).every((key) => required.includes(key) || optional.includes(key)) &&
    required.every((key) => Object.hasOwn(value, key))
  )
}
