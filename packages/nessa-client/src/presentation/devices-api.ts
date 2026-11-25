import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  type DeviceDisk,
  type DeviceMachine,
  type DevicesListResult,
} from "../generated/product.js"

/**
 * The gateway's own machine. Every gateway runs only its own agents
 * (ADR 1417): another computer runs a gateway of its own, so there are no
 * workers, SSH hosts or pool to list.
 */
export type DevicesApi = {
  /**
   * The gateway's own machine (absent when it cannot be read) and where its
   * native listener is bound. Figures are a snapshot; read again to follow
   * them.
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
const MAX_DISKS = 16
const MAX_LISTENING = 64

/** Reads a `devices.list` answer, refusing anything off its schema. */
export function devicesList(value: unknown): DevicesListResult {
  const invalid = () => new TypeError("Invalid devices list")
  if (!isRecord(value) || !onlyKeys(value, [], ["hub", "listening"])) throw invalid()
  if (
    Object.hasOwn(value, "listening") &&
    (typeof value.listening !== "string" || value.listening.length > MAX_LISTENING)
  )
    throw invalid()
  return {
    ...(Object.hasOwn(value, "hub") ? { hub: machine(value.hub) } : {}),
    ...(Object.hasOwn(value, "listening")
      ? { listening: value.listening as string }
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
