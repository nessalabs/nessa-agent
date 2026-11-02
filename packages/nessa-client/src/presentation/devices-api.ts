import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  type DeviceDisk,
  type DeviceMachine,
  type DeviceSshConnect,
  type DeviceSshHost,
  type DeviceWorker,
  type DevicesListResult,
  type DevicesSshHostsResult,
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
   * The machines the gateway's `~/.ssh/config` names that conversations
   * could run on (git servers and hosts already configured left out), and
   * each connection `connectSsh` has under way or ended since the gateway
   * started.
   */
  sshHosts(): Promise<DevicesSshHostsResult>
  /**
   * Install Nessa on `host` over SSH and add it to the gateway's hosts,
   * which restarts the gateway in place. Answered at once with where it
   * stands; `sshHosts` follows it.
   */
  connectSsh(host: string): Promise<DeviceSshConnect>
  /**
   * Save the device new conversations run on when they name none: an SSH
   * host the gateway serves, a paired environment's `peer:` name, or `null`
   * for the gateway's own machine. Answers the saved default.
   */
  setDefault(environment: string | null): Promise<string | null>
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
    sshHosts: async () =>
      devicesSshHosts(await session.request(ProductMethod.DevicesSshHosts, {})),
    connectSsh: async (host) => {
      const answer = await session.request(ProductMethod.DevicesConnectSsh, { host })
      if (!isRecord(answer) || !onlyKeys(answer, ["connecting"], []))
        throw new TypeError("Invalid SSH connection")
      return sshConnect(answer.connecting)
    },
    setDefault: async (environment) => {
      const answer = await session.request(ProductMethod.DevicesSetDefault, {
        environment,
      })
      if (
        !isRecord(answer) ||
        !onlyKeys(answer, ["defaultEnvironment"], []) ||
        (answer.defaultEnvironment !== null && !destination(answer.defaultEnvironment))
      )
        throw new TypeError("Invalid default device")
      return answer.defaultEnvironment as string | null
    },
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
const MAX_WORKERS = 64
const MAX_LISTENING = 64
const ENVIRONMENT = /^peer:[0-9a-f]{64}$/
const STATES = new Set<string>(["online", "draining", "offline"])
const MAX_DESTINATION = 253
const MAX_SSH_HOSTS = 16
const MAX_SUGGESTED = 32
const CONNECT_STATES = new Set<string>([
  "installing",
  "restarting",
  "connected",
  "failed",
])
const CONNECT_REFUSALS = new Set<string>([
  "environment_unreachable",
  "environment_platform_unsupported",
  "environment_install_failed",
  "environment_version_mismatch",
  "devices_host_limit",
  "devices_busy",
  "devices_unavailable",
])

/** Reads a `devices.list` answer, refusing anything off its schema. */
export function devicesList(value: unknown): DevicesListResult {
  const invalid = () => new TypeError("Invalid devices list")
  if (
    !isRecord(value) ||
    !onlyKeys(
      value,
      ["workers"],
      ["hub", "listening", "next", "sshHosts", "defaultEnvironment"],
    )
  )
    throw invalid()
  if (
    Object.hasOwn(value, "defaultEnvironment") &&
    !destination(value.defaultEnvironment)
  )
    throw invalid()
  const sshHosts = value.sshHosts
  if (
    sshHosts !== undefined &&
    (!Array.isArray(sshHosts) || sshHosts.length > MAX_SSH_HOSTS)
  )
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
    ...(Array.isArray(sshHosts) ? { sshHosts: sshHosts.map(sshHost) } : {}),
    ...(Object.hasOwn(value, "defaultEnvironment")
      ? { defaultEnvironment: value.defaultEnvironment as string }
      : {}),
  }
}

/** Reads a `devices.sshHosts` answer, refusing anything off its schema. */
export function devicesSshHosts(value: unknown): DevicesSshHostsResult {
  if (
    !isRecord(value) ||
    !onlyKeys(value, ["suggested", "connecting"], []) ||
    !Array.isArray(value.suggested) ||
    value.suggested.length > MAX_SUGGESTED ||
    !Array.isArray(value.connecting) ||
    value.connecting.length > MAX_SSH_HOSTS
  )
    throw new TypeError("Invalid SSH hosts")
  return {
    suggested: value.suggested.map(sshHost),
    connecting: value.connecting.map(sshConnect),
  }
}

function destination(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    new TextEncoder().encode(value).length <= MAX_DESTINATION
  )
}

function sshHost(value: unknown): DeviceSshHost {
  if (
    !isRecord(value) ||
    !onlyKeys(value, ["host"], ["hostName", "user", "port"]) ||
    !destination(value.host) ||
    (Object.hasOwn(value, "hostName") && !destination(value.hostName)) ||
    (Object.hasOwn(value, "user") && !destination(value.user)) ||
    (Object.hasOwn(value, "port") &&
      (!count(value.port, 65_535) || (value.port as number) < 1))
  )
    throw new TypeError("Invalid SSH host")
  return {
    host: value.host,
    ...(Object.hasOwn(value, "hostName") ? { hostName: value.hostName as string } : {}),
    ...(Object.hasOwn(value, "user") ? { user: value.user as string } : {}),
    ...(Object.hasOwn(value, "port") ? { port: value.port as number } : {}),
  }
}

function sshConnect(value: unknown): DeviceSshConnect {
  if (
    !isRecord(value) ||
    !onlyKeys(value, ["host", "state"], ["refusal"]) ||
    !destination(value.host) ||
    typeof value.state !== "string" ||
    !CONNECT_STATES.has(value.state) ||
    (value.state === "failed") !== Object.hasOwn(value, "refusal") ||
    (Object.hasOwn(value, "refusal") &&
      (typeof value.refusal !== "string" || !CONNECT_REFUSALS.has(value.refusal)))
  )
    throw new TypeError("Invalid SSH connection")
  return {
    host: value.host,
    state: value.state as DeviceSshConnect["state"],
    ...(Object.hasOwn(value, "refusal")
      ? { refusal: value.refusal as NonNullable<DeviceSshConnect["refusal"]> }
      : {}),
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
