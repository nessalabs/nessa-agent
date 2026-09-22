import { describe, expect, it, vi } from "vitest"
import { readFileSync } from "node:fs"
import { chmod, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises"
import { execFile } from "node:child_process"
import { promisify } from "node:util"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { NessaEndpointDiscoveryError } from "../application/gateway-endpoint.js"
import {
  LocalGatewayEndpointSource,
  nodeGatewayEndpointSource,
} from "./local-gateway-endpoint.js"

const record = {
  webSocketUrl: "ws://127.0.0.1:9137",
  endpointInstance: "5485b918-1eeb-4a4a-ad1d-9fdc70dfa231",
  processId: 4711,
  runtimeFingerprint: "a".repeat(64),
  serviceGeneration: "b".repeat(64),
  runtimeInstance: "5485b918-1eeb-4a4a-ad1d-9fdc70dfa231",
  runtimeProcessId: 4711,
}

function health(overrides: Record<string, string> = {}) {
  return new Response(null, {
    status: 200,
    headers: {
      "x-nessa-endpoint-instance": record.endpointInstance,
      "x-nessa-endpoint-process-id": String(record.processId),
      "x-nessa-runtime-fingerprint": record.runtimeFingerprint,
      "x-nessa-service-generation": record.serviceGeneration,
      "x-nessa-runtime-instance": record.runtimeInstance,
      "x-nessa-process-id": String(record.runtimeProcessId),
      ...overrides,
    },
  })
}

function source(text: string | undefined, response = health()) {
  const request = vi.fn().mockResolvedValue(response)
  return {
    endpoint: new LocalGatewayEndpointSource(
      { path: async () => "/private/logs/gateway-endpoint.json", read: async () => text },
      request,
    ),
    request,
  }
}

describe("local gateway endpoint discovery", () => {
  it("parses the canonical publication shared with Rust", async () => {
    const fixture = readFileSync(
      new URL("../../../../protocol/fixtures/gateway-endpoint.json", import.meta.url),
      "utf8",
    )
    const canonical = fixture.trimEnd()
    expect(canonical).toBe(JSON.stringify(record))
    await expect(source(canonical).endpoint.load({ stage: "prod" })).resolves.toBe(
      record.webSocketUrl,
    )
  })

  it("accepts a custom port only after every published identity field matches health", async () => {
    const { endpoint, request } = source(JSON.stringify(record))
    await expect(endpoint.load({ stage: "prod" })).resolves.toBe(record.webSocketUrl)
    expect(request).toHaveBeenCalledWith(
      "http://127.0.0.1:9137/health",
      expect.any(AbortSignal),
    )
  })

  it("accepts a standalone publication without inventing managed runtime identity", async () => {
    const standalone = {
      webSocketUrl: "ws://[::1]:8129",
      endpointInstance: record.endpointInstance,
      processId: record.processId,
    }
    const { endpoint } = source(
      JSON.stringify(standalone),
      new Response(null, {
        status: 200,
        headers: {
          "x-nessa-endpoint-instance": record.endpointInstance,
          "x-nessa-endpoint-process-id": String(record.processId),
        },
      }),
    )
    await expect(endpoint.load({ stage: "dev" })).resolves.toBe("ws://[::1]:8129")
  })

  it("refuses managed health identity for a standalone publication", async () => {
    const standalone = {
      webSocketUrl: "ws://127.0.0.1:8129",
      endpointInstance: record.endpointInstance,
      processId: record.processId,
    }
    await expect(
      source(JSON.stringify(standalone), health()).endpoint.load({ stage: "dev" }),
    ).rejects.toBeInstanceOf(NessaEndpointDiscoveryError)
  })

  it("treats an absent or unreadable publication as no publication", async () => {
    await expect(
      source(undefined).endpoint.load({ stage: "dev" }),
    ).resolves.toBeUndefined()
    const endpoint = new LocalGatewayEndpointSource(
      {
        path: async () => "/private/logs/gateway-endpoint.json",
        read: async () => {
          throw new Error("disk unavailable")
        },
      },
      vi.fn(),
    )
    await expect(endpoint.load({ stage: "dev" })).resolves.toBeUndefined()
  })

  it.runIf(process.platform !== "win32")(
    "reads the real stage and instance namespace with private file checks",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "nessa-endpoint-"))
      try {
        const logs = join(root, "ci", "instances", "worker-7", "logs")
        await mkdir(logs, { recursive: true, mode: 0o700 })
        await writeFile(join(logs, "gateway-endpoint.json"), JSON.stringify(record), {
          mode: 0o600,
        })
        const endpoint = await nodeGatewayEndpointSource({
          dataDir: root,
          instance: "worker-7",
          uid: process.getuid?.(),
          request: async () => health(),
        })
        await expect(endpoint?.load({ stage: "ci" })).resolves.toBe(record.webSocketUrl)
      } finally {
        await rm(root, { recursive: true })
      }
    },
  )

  it.runIf(process.platform !== "win32")(
    "refuses a symlinked publication as unsafe",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "nessa-endpoint-link-"))
      try {
        const logs = join(root, "dev", "logs")
        await mkdir(logs, { recursive: true, mode: 0o700 })
        const outside = join(root, "outside.json")
        await writeFile(outside, JSON.stringify(record), { mode: 0o600 })
        await promisify(execFile)("ln", [
          "-s",
          outside,
          join(logs, "gateway-endpoint.json"),
        ])
        const endpoint = await nodeGatewayEndpointSource({
          dataDir: root,
          uid: process.getuid?.(),
          request: async () => health(),
        })
        await expect(endpoint?.load({ stage: "dev" })).rejects.toBeInstanceOf(
          NessaEndpointDiscoveryError,
        )
      } finally {
        await rm(root, { recursive: true })
      }
    },
  )

  it.runIf(process.platform !== "win32")(
    "refuses a fifo publication without waiting for a writer",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "nessa-endpoint-fifo-"))
      try {
        const logs = join(root, "dev", "logs")
        await mkdir(logs, { recursive: true, mode: 0o700 })
        await promisify(execFile)("mkfifo", [join(logs, "gateway-endpoint.json")])
        const endpoint = await nodeGatewayEndpointSource({
          dataDir: root,
          uid: process.getuid?.(),
          request: async () => health(),
        })
        await expect(endpoint?.load({ stage: "dev" })).rejects.toBeInstanceOf(
          NessaEndpointDiscoveryError,
        )
      } finally {
        await rm(root, { recursive: true })
      }
    },
  )

  it.runIf(process.platform !== "win32")(
    "refuses a matching record reached through a symlinked namespace",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "nessa-endpoint-ancestor-"))
      const outside = await mkdtemp(join(tmpdir(), "nessa-endpoint-outside-"))
      try {
        await chmod(outside, 0o700)
        const logs = join(outside, "logs")
        await mkdir(logs, { mode: 0o700 })
        await writeFile(join(logs, "gateway-endpoint.json"), JSON.stringify(record), {
          mode: 0o600,
        })
        await symlink(outside, join(root, "dev"))
        const request = vi.fn(async () => health())
        const endpoint = await nodeGatewayEndpointSource({
          dataDir: root,
          uid: process.getuid?.(),
          request,
        })
        await expect(endpoint?.load({ stage: "dev" })).rejects.toBeInstanceOf(
          NessaEndpointDiscoveryError,
        )
        expect(request).not.toHaveBeenCalled()
      } finally {
        await rm(root, { recursive: true })
        await rm(outside, { recursive: true })
      }
    },
  )

  it.runIf(process.platform !== "win32")(
    "refuses a private root acquired through an attacker-writable parent",
    async () => {
      const fixture = await mkdtemp(join(tmpdir(), "nessa-endpoint-parent-"))
      try {
        const parent = join(fixture, "writable")
        const root = join(parent, "data")
        const logs = join(root, "dev", "logs")
        await mkdir(logs, { recursive: true, mode: 0o700 })
        await chmod(parent, 0o777)
        await writeFile(join(logs, "gateway-endpoint.json"), JSON.stringify(record), {
          mode: 0o600,
        })
        const endpoint = await nodeGatewayEndpointSource({
          dataDir: root,
          uid: process.getuid?.(),
          request: async () => health(),
        })
        await expect(endpoint?.load({ stage: "dev" })).rejects.toBeInstanceOf(
          NessaEndpointDiscoveryError,
        )
      } finally {
        await rm(fixture, { recursive: true })
      }
    },
  )

  it.runIf(process.platform !== "win32")(
    "refuses a namespace with unsafe mode or owner evidence",
    async () => {
      const root = await mkdtemp(join(tmpdir(), "nessa-endpoint-private-"))
      try {
        const logs = join(root, "dev", "logs")
        await mkdir(logs, { recursive: true, mode: 0o700 })
        await writeFile(join(logs, "gateway-endpoint.json"), JSON.stringify(record), {
          mode: 0o600,
        })
        const options = {
          dataDir: root,
          request: async () => health(),
        }
        await chmod(logs, 0o755)
        const publicDirectory = await nodeGatewayEndpointSource({
          ...options,
          uid: process.getuid?.(),
        })
        await expect(publicDirectory?.load({ stage: "dev" })).rejects.toBeInstanceOf(
          NessaEndpointDiscoveryError,
        )
        await chmod(logs, 0o700)
        const wrongOwner = await nodeGatewayEndpointSource({
          ...options,
          uid: (process.getuid?.() ?? 0) + 1,
        })
        await expect(wrongOwner?.load({ stage: "dev" })).rejects.toBeInstanceOf(
          NessaEndpointDiscoveryError,
        )
      } finally {
        await rm(root, { recursive: true })
      }
    },
  )

  it.each([
    "not json",
    JSON.stringify({ ...record, webSocketUrl: "ws://example.com:9137" }),
    JSON.stringify({ ...record, webSocketUrl: "wss://127.0.0.1:9137" }),
    JSON.stringify({ ...record, webSocketUrl: "ws://127.0.0.1:0" }),
    JSON.stringify({ ...record, runtimeInstance: undefined }),
    JSON.stringify({
      ...record,
      runtimeInstance: "38baa4b2-bd37-45fa-aeda-d7b04059da21",
    }),
    JSON.stringify({ ...record, runtimeProcessId: 9 }),
    `${JSON.stringify(record)}\n`,
    `{"webSocketUrl":"ws://127.0.0.1:1","webSocketUrl":"ws://127.0.0.1:9137","endpointInstance":"${record.endpointInstance}","processId":4711}`,
  ])("refuses malformed present publication before health: %s", async (text) => {
    const { endpoint, request } = source(text)
    await expect(endpoint.load({ stage: "dev" })).rejects.toBeInstanceOf(
      NessaEndpointDiscoveryError,
    )
    expect(request).not.toHaveBeenCalled()
  })

  it.each([
    ["x-nessa-endpoint-instance", "38baa4b2-bd37-45fa-aeda-d7b04059da21"],
    ["x-nessa-endpoint-process-id", "4712"],
    ["x-nessa-runtime-fingerprint", "c".repeat(64)],
    ["x-nessa-service-generation", "d".repeat(64)],
    ["x-nessa-runtime-instance", "38baa4b2-bd37-45fa-aeda-d7b04059da21"],
    ["x-nessa-process-id", "4712"],
  ])("refuses stale or mismatched health field %s", async (header, value) => {
    const { endpoint } = source(JSON.stringify(record), health({ [header]: value }))
    await expect(endpoint.load({ stage: "dev" })).rejects.toBeInstanceOf(
      NessaEndpointDiscoveryError,
    )
  })

  it("refuses an endpoint that cannot answer health", async () => {
    const endpoint = new LocalGatewayEndpointSource(
      { path: async () => "/record", read: async () => JSON.stringify(record) },
      async () => {
        throw new Error("connection refused")
      },
    )
    await expect(endpoint.load({ stage: "dev" })).rejects.toBeInstanceOf(
      NessaEndpointDiscoveryError,
    )
  })
})
