/**
 * The Devices view's words: what `devices.list`, the Boat connection and
 * the linked devices say, read into cards. Pure; the gateway decides every
 * fact, this only says it.
 */

/** A machine as the hub reports it (`devices.list`'s `DeviceMachine`). */
export interface DeviceMachine {
  readonly hostname: string
  readonly os: string
  readonly osVersion: string
  readonly arch: string
  readonly cpus: number
  readonly cpuPermille: number
  readonly memoryTotalBytes: number
  readonly memoryUsedBytes: number
  readonly diskTotalBytes: number
  readonly diskUsedBytes: number
  /** Each local volume the disk figures add up; absent from older hubs. */
  readonly disks?: readonly DeviceDisk[]
}

/** One volume (`devices.list`'s `DeviceDisk`). */
export interface DeviceDisk {
  readonly mount: string
  readonly totalBytes: number
  readonly usedBytes: number
}

/** A worker as the hub lists it (`devices.list`'s `DeviceWorker`). */
export interface DeviceWorker {
  readonly environment: string
  readonly state: "online" | "draining" | "offline"
  readonly agents: number
  readonly machine?: DeviceMachine
  readonly lastSeenMs?: number
}

/** Which drawing a device gets (`DeviceFigure`'s kinds). */
export type DeviceArt = "desktop" | "laptop" | "server" | "monitor" | "phone" | "cloud"

/** How a device stands, as its dot and word say it. */
export type DeviceStatus = "online" | "draining" | "offline" | "ready" | "away"

/** The hub: the machine this gateway runs on. */
export interface HubCard {
  readonly name: string
  readonly system: string
  readonly cpu: Usage
  readonly memory: Usage
  readonly disk: Usage
}

/** A share of something, and how to say it. */
export interface Usage {
  /** 0–1. */
  readonly fraction: number
  /** e.g. "23%" or "14 GB". */
  readonly value: string
  /** e.g. "of 48 GB"; empty for processor use. */
  readonly of: string
}

/** One worker, as a card and as its own page. */
export interface WorkerCard {
  readonly id: string
  readonly name: string
  readonly art: DeviceArt
  readonly system: string
  readonly status: DeviceStatus
  readonly agents: number
  /** Usage while it serves; absent offline. */
  readonly cpu?: Usage
  readonly memory?: Usage
  readonly disk?: Usage
  /** Each volume while it serves, when it has more than one. */
  readonly disks?: readonly DiskUsage[]
  readonly cpus?: number
  readonly lastSeenMs?: number
  readonly environment: string
}

/** One volume and how full it is. */
export interface DiskUsage {
  readonly mount: string
  readonly usage: Usage
}

const GB = 1024 ** 3

function usage(used: number, total: number): Usage {
  const fraction = total > 0 ? Math.min(1, used / total) : 0
  return { fraction, value: bytes(used), of: total > 0 ? `of ${bytes(total)}` : "" }
}

/** Bytes as people say them: GB from one up, else MB. */
export function bytes(value: number): string {
  if (value >= 1024 * GB) return `${trim(value / (1024 * GB))} TB`
  if (value >= GB) return `${Math.round(value / GB)} GB`
  return `${Math.round(value / 1024 ** 2)} MB`
}

function trim(value: number): string {
  return value >= 10 ? String(Math.round(value)) : value.toFixed(1).replace(/\.0$/, "")
}

function cpu(machine: DeviceMachine): Usage {
  const fraction = Math.min(1, machine.cpuPermille / 1000)
  return { fraction, value: `${Math.round(fraction * 100)}%`, of: "" }
}

/** The system's name as people say it. */
export function systemName(machine: DeviceMachine): string {
  const family =
    machine.os === "macos"
      ? "macOS"
      : machine.os === "windows"
        ? "Windows"
        : machine.os === "linux"
          ? "Linux"
          : machine.os || "Unknown system"
  const version = machine.osVersion ? ` ${machine.osVersion}` : ""
  const chip = machine.arch === "aarch64" ? " · Apple silicon" : ""
  return `${family}${version}${machine.os === "macos" ? chip : ""}`
}

/** The drawing a worker gets: what its system most likely is. */
export function artOf(machine: DeviceMachine | undefined): DeviceArt {
  switch (machine?.os) {
    case "macos":
      return "laptop"
    case "windows":
      return "monitor"
    default:
      return "server"
  }
}

export function hubCard(machine: DeviceMachine): HubCard {
  return {
    name: machine.hostname || "This Mac",
    system: systemName(machine),
    cpu: cpu(machine),
    memory: usage(machine.memoryUsedBytes, machine.memoryTotalBytes),
    disk: usage(machine.diskUsedBytes, machine.diskTotalBytes),
  }
}

/** A worker's name: its own, or the start of its key while it has said none. */
export function workerName(worker: DeviceWorker): string {
  const named = worker.machine?.hostname.trim()
  if (named) return named
  return `Worker ${worker.environment.slice("peer:".length, "peer:".length + 6)}`
}

export function workerCard(worker: DeviceWorker): WorkerCard {
  const machine = worker.machine
  const serving = worker.state !== "offline"
  return {
    id: worker.environment.slice("peer:".length),
    environment: worker.environment,
    name: workerName(worker),
    art: artOf(machine),
    system: machine ? systemName(machine) : "Has not reported yet",
    status: worker.state,
    agents: serving ? worker.agents : 0,
    ...(serving && machine
      ? {
          cpu: cpu(machine),
          memory: usage(machine.memoryUsedBytes, machine.memoryTotalBytes),
          disk: usage(machine.diskUsedBytes, machine.diskTotalBytes),
          ...(machine.disks && machine.disks.length > 1
            ? {
                disks: machine.disks.map((disk) => ({
                  mount: disk.mount,
                  usage: usage(disk.usedBytes, disk.totalBytes),
                })),
              }
            : {}),
        }
      : {}),
    ...(machine ? { cpus: machine.cpus } : {}),
    ...(worker.lastSeenMs !== undefined ? { lastSeenMs: worker.lastSeenMs } : {}),
  }
}

/** Online ones first, then draining, then offline; by name within each. */
export function sortWorkers(cards: readonly WorkerCard[]): WorkerCard[] {
  const rank: Record<DeviceStatus, number> = {
    online: 0,
    ready: 0,
    draining: 1,
    away: 2,
    offline: 3,
  }
  return [...cards].sort(
    (a, b) => rank[a.status] - rank[b.status] || a.name.localeCompare(b.name),
  )
}

/** "Online", "Finishing up", … */
export function statusWord(status: DeviceStatus): string {
  switch (status) {
    case "online":
      return "Online"
    case "draining":
      return "Finishing up"
    case "offline":
      return "Offline"
    case "ready":
      return "Ready"
    case "away":
      return "Away"
  }
}

/** "1 agent", "3 agents", "No agents". */
export function agentsWord(count: number): string {
  if (count === 0) return "No agents"
  return count === 1 ? "1 agent" : `${count} agents`
}

/** How long ago, said plainly: "just now", "5 min ago", "2 h ago", "3 days ago". */
export function ago(thenMs: number, nowMs: number): string {
  const seconds = Math.max(0, Math.round((nowMs - thenMs) / 1000))
  if (seconds < 60) return "just now"
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  const days = Math.round(hours / 24)
  return days === 1 ? "yesterday" : `${days} days ago`
}

/** The page's one line under its title. */
export function summary(hub: string, workers: readonly WorkerCard[]): string {
  const online = workers.filter((worker) => worker.status !== "offline").length
  const agents = workers.reduce((sum, worker) => sum + worker.agents, 0)
  const total = workers.length
  if (total === 0) return `${hub} is your hub. Add a computer to run agents on it too.`
  const devices = `${online} of ${total} ${total === 1 ? "computer" : "computers"} online`
  return `${hub} is your hub. ${devices}, ${agentsWord(agents).toLowerCase()} on them.`
}
