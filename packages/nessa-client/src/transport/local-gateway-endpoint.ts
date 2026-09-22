import {
  NessaEndpointDiscoveryError,
  type GatewayEndpointSource,
} from "../application/gateway-endpoint.js"
import { isLoopbackWebSocketUrl } from "../application/resolve-options.js"

const FILE = "gateway-endpoint.json"
const MAX_RECORD_BYTES = 16 * 1024
const HEALTH_TIMEOUT_MS = 1_000
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const sha256 = /^[0-9a-f]{64}$/

type EndpointRecord = {
  webSocketUrl: string
  endpointInstance: string
  processId: number
  runtimeFingerprint?: string
  serviceGeneration?: string
  runtimeInstance?: string
  runtimeProcessId?: number
}

type EndpointFile = {
  read(path: string): Promise<string | undefined>
  path(stage: string): Promise<string>
}

type HealthRequest = (url: string, signal: AbortSignal) => Promise<Response>

function own(object: object, key: string): boolean {
  return Object.hasOwn(object, key)
}

function recordOf(value: unknown): EndpointRecord {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error()
  const object = value as Record<string, unknown>
  const allowed = new Set([
    "webSocketUrl",
    "endpointInstance",
    "processId",
    "runtimeFingerprint",
    "serviceGeneration",
    "runtimeInstance",
    "runtimeProcessId",
  ])
  if (Object.keys(object).some((key) => !allowed.has(key))) throw new Error()
  if (
    typeof object.webSocketUrl !== "string" ||
    typeof object.endpointInstance !== "string" ||
    !uuid.test(object.endpointInstance) ||
    !Number.isSafeInteger(object.processId) ||
    Number(object.processId) <= 0 ||
    Number(object.processId) > 0xffff_ffff
  )
    throw new Error()
  if (!isLoopbackWebSocketUrl(object.webSocketUrl)) throw new Error()
  const parsed = new URL(object.webSocketUrl)
  if (parsed.pathname !== "/" || parsed.search || parsed.hash) throw new Error()
  const managedKeys = [
    "runtimeFingerprint",
    "serviceGeneration",
    "runtimeInstance",
    "runtimeProcessId",
  ]
  const managed = managedKeys.filter((key) => own(object, key)).length
  if (managed !== 0 && managed !== managedKeys.length) throw new Error()
  if (
    managed === managedKeys.length &&
    (typeof object.runtimeFingerprint !== "string" ||
      !sha256.test(object.runtimeFingerprint) ||
      typeof object.serviceGeneration !== "string" ||
      !sha256.test(object.serviceGeneration) ||
      typeof object.runtimeInstance !== "string" ||
      !uuid.test(object.runtimeInstance) ||
      object.runtimeInstance !== object.endpointInstance ||
      !Number.isSafeInteger(object.runtimeProcessId) ||
      object.runtimeProcessId !== object.processId)
  )
    throw new Error()
  return object as EndpointRecord
}

function healthUrl(webSocketUrl: string): string {
  const url = new URL(webSocketUrl)
  url.protocol = url.protocol === "wss:" ? "https:" : "http:"
  url.pathname = "/health"
  return url.toString()
}

/** Node private-file and HTTP adapter for local endpoint discovery. */
export class LocalGatewayEndpointSource implements GatewayEndpointSource {
  constructor(
    private readonly file: EndpointFile,
    private readonly request: HealthRequest,
  ) {}

  async load({ stage }: { stage: string }): Promise<string | undefined> {
    let text: string | undefined
    try {
      text = await this.file.read(await this.file.path(stage))
    } catch (error) {
      if (error instanceof NessaEndpointDiscoveryError) throw error
      return undefined
    }
    if (text === undefined) return undefined
    let record: EndpointRecord
    try {
      if (new TextEncoder().encode(text).byteLength > MAX_RECORD_BYTES) throw new Error()
      record = recordOf(JSON.parse(text))
      // The server writes one compact record in this field order. Requiring its
      // canonical spelling also rejects duplicate JSON keys, which JSON.parse
      // would otherwise collapse before validation.
      if (JSON.stringify(record) !== text) throw new Error()
    } catch {
      throw new NessaEndpointDiscoveryError("The published gateway endpoint is malformed")
    }

    const timeout = new AbortController()
    const elapsed = setTimeout(() => timeout.abort(), HEALTH_TIMEOUT_MS)
    let response: Response
    try {
      response = await this.request(healthUrl(record.webSocketUrl), timeout.signal)
    } catch {
      throw new NessaEndpointDiscoveryError(
        "The published gateway endpoint did not answer health",
      )
    } finally {
      clearTimeout(elapsed)
    }
    const matches =
      response.status === 200 &&
      response.headers.get("x-nessa-endpoint-instance") === record.endpointInstance &&
      response.headers.get("x-nessa-endpoint-process-id") === String(record.processId) &&
      response.headers.get("x-nessa-runtime-fingerprint") ===
        (record.runtimeFingerprint ?? null) &&
      response.headers.get("x-nessa-service-generation") ===
        (record.serviceGeneration ?? null) &&
      response.headers.get("x-nessa-runtime-instance") ===
        (record.runtimeInstance ?? null) &&
      response.headers.get("x-nessa-process-id") ===
        (record.runtimeProcessId === undefined ? null : String(record.runtimeProcessId))
    if (!matches)
      throw new NessaEndpointDiscoveryError(
        "The published gateway endpoint belongs to a different process",
      )
    return record.webSocketUrl
  }
}

/** Compose the Node adapter without importing Node modules into browser bundles. */
export async function nodeGatewayEndpointSource(options: {
  dataDir?: string
  home?: string
  instance?: string
  uid?: number
  request?: HealthRequest
}): Promise<GatewayEndpointSource | undefined> {
  if (typeof process === "undefined" || !process.versions?.node) return undefined
  const fsModule = "node:fs/promises"
  const pathModule = "node:path"
  const constantsModule = "node:fs"
  const fs = (await import(
    /* @vite-ignore */ fsModule
  )) as typeof import("node:fs/promises")
  const path = (await import(/* @vite-ignore */ pathModule)) as typeof import("node:path")
  const { constants } = (await import(
    /* @vite-ignore */ constantsModule
  )) as typeof import("node:fs")
  const segment = (value: string) => /^[A-Za-z0-9_-]+$/.test(value)
  const base =
    options.dataDir ?? (options.home ? path.join(options.home, ".nessa") : undefined)
  const file: EndpointFile = {
    async path(stage) {
      if (
        !base ||
        !path.isAbsolute(base) ||
        !segment(stage) ||
        (options.instance !== undefined && !segment(options.instance))
      )
        throw new Error("invalid local endpoint namespace")
      let root = stage === "prod" ? base : path.join(base, stage)
      if (options.instance) root = path.join(root, "instances", options.instance)
      return path.join(root, "logs", FILE)
    },
    async read(filePath) {
      if (process.platform === "win32") {
        try {
          const { windowsPrivateFile } = await import("./windows-private-file.js")
          return await windowsPrivateFile("read", filePath)
        } catch {
          return undefined
        }
      }
      let handle: import("node:fs/promises").FileHandle
      try {
        handle = await fs.open(filePath, constants.O_RDONLY | constants.O_NOFOLLOW)
      } catch {
        return undefined
      }
      try {
        const stat = await handle.stat()
        if (
          !stat.isFile() ||
          stat.nlink !== 1 ||
          options.uid === undefined ||
          stat.uid !== options.uid ||
          (stat.mode & 0o077) !== 0 ||
          stat.size < 1 ||
          stat.size > MAX_RECORD_BYTES
        )
          throw new NessaEndpointDiscoveryError(
            "The published gateway endpoint file is not private",
          )
        return await handle.readFile("utf8")
      } finally {
        await handle.close()
      }
    },
  }
  return new LocalGatewayEndpointSource(
    file,
    options.request ??
      ((url, signal) =>
        globalThis.fetch(url, { signal, redirect: "error", cache: "no-store" })),
  )
}
