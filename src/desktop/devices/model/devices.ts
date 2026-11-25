/**
 * The Devices view's words: what `devices.list`, the Boat connection and
 * the linked devices say, read into cards. Every gateway runs only its own
 * agents (ADR 1417), so the hub is the one computer listed. Pure; the gateway decides every
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

/** How a device stands, as its dot and word say it. */
export type DeviceStatus = "online" | "offline" | "ready"

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

export function hubCard(machine: DeviceMachine): HubCard {
  return {
    name: machine.hostname || "This Mac",
    system: systemName(machine),
    cpu: cpu(machine),
    memory: usage(machine.memoryUsedBytes, machine.memoryTotalBytes),
    disk: usage(machine.diskUsedBytes, machine.diskTotalBytes),
  }
}

/** "Online", "Ready", … */
export function statusWord(status: DeviceStatus): string {
  switch (status) {
    case "online":
      return "Online"
    case "offline":
      return "Offline"
    case "ready":
      return "Ready"
  }
}
