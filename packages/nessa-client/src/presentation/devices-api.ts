import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  type DeviceMachine,
  type DeviceWorker,
  type DevicesListResult,
} from "../generated/product.js"

/** The hub's devices: the machine the gateway runs on and its workers. */
export type DevicesApi = {
  /**
   * The gateway's own machine (absent when it cannot be read) and the
   * paired environments that work for it, connected now or seen before:
   * at most 64, online first, with `omitted` counting the rest. Figures
   * are a snapshot; read again to follow them.
   */
  list(): Promise<DevicesListResult>
}

export function createDevicesApi(session: RpcRequester): DevicesApi {
  return {
    list: async () => devicesList(await session.request(ProductMethod.DevicesList, {})),
  }
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
const MAX_WORKERS = 64
const MAX_LISTENING = 64
const ENVIRONMENT = /^peer:[0-9a-f]{64}$/
const STATES = new Set<string>(["online", "draining", "offline"])

/** Reads a `devices.list` answer, refusing anything off its schema. */
export function devicesList(value: unknown): DevicesListResult {
  const invalid = () => new TypeError("Invalid devices list")
  if (!isRecord(value) || !onlyKeys(value, ["workers", "omitted"], ["hub", "listening"]))
    throw invalid()
  if (
    Object.hasOwn(value, "listening") &&
    (typeof value.listening !== "string" || value.listening.length > MAX_LISTENING)
  )
    throw invalid()
  const workers = value.workers
  if (!Array.isArray(workers) || workers.length > MAX_WORKERS || !count(value.omitted))
    throw invalid()
  return {
    ...(Object.hasOwn(value, "hub") ? { hub: machine(value.hub) } : {}),
    ...(Object.hasOwn(value, "listening")
      ? { listening: value.listening as string }
      : {}),
    workers: workers.map(worker),
    omitted: value.omitted as number,
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
  if (!isRecord(value) || !onlyKeys(value, MACHINE_KEYS, []))
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
  return Object.fromEntries(
    MACHINE_KEYS.map((key) => [key, value[key]]),
  ) as unknown as DeviceMachine
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
