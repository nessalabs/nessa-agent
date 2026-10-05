/**
 * The window's client read into the Integrations model: each refusal and
 * problem in the model's words, the connection followed, and a save sent as
 * the wire takes it.
 */
import {
  McpServerProblemCode,
  McpServersErrorCode,
  NessaMcpServersError,
  NessaRpcError,
  type ConnectionState,
  type McpServersApi,
  type ProductSessionReady,
} from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import {
  failureOf,
  mcpServersGateway,
  type McpServersClient,
} from "./mcp-servers-gateway"

const refusal = (code: string, details?: unknown) =>
  new NessaMcpServersError("mcpServers.save", new NessaRpcError(code, "refused", details))

describe("failureOf", () => {
  it("reads every refusal code into the model's words", () => {
    const kinds = Object.values(McpServersErrorCode).map(
      (code) => failureOf(refusal(code)).kind,
    )
    expect(kinds).toEqual([
      "notConfigured",
      "invalid",
      "reservedName",
      "notFound",
      "revisionConflict",
      "busy",
      "configInvalid",
      "configTooLarge",
      "storageUnavailable",
      "auditUnavailable",
      "stopping",
      "startFailed",
      "timedOut",
      "gone",
      "malformed",
      "remoteError",
    ])
  })

  it("reads every problem, with its server and variable", () => {
    const problems = Object.values(McpServerProblemCode).map((problem) =>
      failureOf(refusal("mcp_servers_invalid", { problem, server: "S", name: "N" })),
    )
    expect(problems.map((each) => each.kind === "invalid" && each.problem)).toEqual([
      "tooMany",
      "duplicateName",
      "name",
      "command",
      "arguments",
      "environmentName",
      "reservedEnvironmentName",
      "environmentValue",
      "environmentValueMissing",
      "environmentNameRepeated",
    ])
    expect(problems[0]).toEqual({
      kind: "invalid",
      problem: "tooMany",
      server: "S",
      name: "N",
    })
  })

  it("keeps an audit refusal's applied and its code in the window's words, and a remote error's message", () => {
    expect(
      failureOf(
        refusal("audit_unavailable", { applied: true, code: "mcp_servers_busy" }),
      ),
    ).toEqual({ kind: "auditUnavailable", applied: true, cause: "busy" })
    expect(
      failureOf(
        refusal("audit_unavailable", {
          applied: true,
          code: "mcp_servers_storage_unavailable",
        }),
      ),
    ).toEqual({ kind: "auditUnavailable", applied: true, cause: "storageUnavailable" })
    expect(
      failureOf(refusal("mcp_server_remote_error", { code: 1, message: "m" })),
    ).toEqual({ kind: "remoteError", code: 1, message: "m" })
  })

  it.each([true, false])("keeps a storage refusal's applied (%s)", (applied) => {
    expect(failureOf(refusal("mcp_servers_storage_unavailable", { applied }))).toEqual({
      kind: "storageUnavailable",
      applied,
    })
  })

  it("reads a storage refusal without its details as not knowing whether it applied", () => {
    expect(failureOf(refusal("mcp_servers_storage_unavailable"))).toEqual({
      kind: "storageUnavailable",
    })
  })

  it("is forbidden for the session's refusal, unanswered for anything else", () => {
    expect(failureOf(refusal("forbidden"))).toEqual({ kind: "forbidden" })
    expect(failureOf(refusal("constructor"))).toEqual({ kind: "unanswered" })
    expect(failureOf(new Error("lost"))).toEqual({ kind: "unanswered" })
  })
})

function client(grants: string[], api: Partial<McpServersApi> = {}) {
  let state: ConnectionState = { status: "connected" }
  const handlers = new Set<(state: ConnectionState) => void>()
  const value: McpServersClient = {
    mcpServers: api as McpServersApi,
    productSession: {
      grants: grants.map((action) => ({
        action,
        resource: { organizationId: "o", id: "r" },
      })),
    } as unknown as ProductSessionReady,
    get connectionState() {
      return state
    },
    onConnectionStateChange(handler) {
      handlers.add(handler)
      return () => handlers.delete(handler)
    },
  }
  return {
    value,
    set(next: ConnectionState) {
      state = next
      for (const handler of [...handlers]) handler(next)
    },
    handlers,
  }
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

describe("mcpServersGateway", () => {
  it("follows the connection and whether this credential may manage", async () => {
    const admin = client(["credential.manage"])
    const timers: (() => void)[] = []
    const gateway = mcpServersGateway({
      connected: () => Promise.resolve(admin.value),
      after: (_, run) => {
        timers.push(run)
        return () => {}
      },
    })
    const seen: unknown[] = []
    const stop = gateway.follow((event) => seen.push(event))
    await flush()
    admin.set({ status: "reconnecting", attempt: 1, error: new Error() as never })
    admin.set({ status: "connected" })
    expect(seen).toEqual([
      { type: "connected", mayManage: true },
      { type: "unreachable" },
      { type: "connected", mayManage: true },
    ])
    stop()
    expect(admin.handlers.size).toBe(0)
  })

  it("is unreachable while no client connects, and tries again", async () => {
    const reader = client(["conversation.read"])
    let attempts = 0
    const timers: (() => void)[] = []
    const gateway = mcpServersGateway({
      connected: () =>
        ++attempts === 1
          ? Promise.reject(new Error("no"))
          : Promise.resolve(reader.value),
      after: (_, run) => {
        timers.push(run)
        return () => {}
      },
    })
    const seen: unknown[] = []
    gateway.follow((event) => seen.push(event))
    await flush()
    expect(seen).toEqual([{ type: "unreachable" }])
    timers.shift()!()
    await flush()
    expect(seen).toEqual([
      { type: "unreachable" },
      { type: "connected", mayManage: false },
    ])
  })

  it("sends a save as the wire takes it, a stdio server", async () => {
    const save = vi.fn(() => Promise.resolve({ revision: "r2" }))
    const gateway = mcpServersGateway({
      connected: () => Promise.resolve(client([], { save }).value),
      after: () => () => {},
    })
    await expect(
      gateway.save({
        revision: "r1",
        previousName: "a",
        server: {
          name: "b",
          command: "/c",
          args: ["x"],
          env: [{ name: "K", value: null }],
          enabled: false,
        },
      }),
    ).resolves.toEqual({ ok: true, value: undefined })
    expect(save).toHaveBeenCalledWith({
      revision: "r1",
      previousName: "a",
      server: {
        kind: "stdio",
        name: "b",
        command: "/c",
        args: ["x"],
        env: [{ name: "K", value: null }],
        enabled: false,
      },
    })
  })

  it("reads an inspection's CSP lists that are not empty, and the permissions asked for", async () => {
    const inspect = vi.fn(() =>
      Promise.resolve({
        complete: true,
        tools: [
          {
            name: "show_chart",
            readOnlyHint: true,
            ui: {
              uri: "ui://c",
              csp: {
                connectDomains: ["https://a"],
                resourceDomains: [],
                frameDomains: [],
                baseUriDomains: [],
              },
              permissions: {
                camera: false,
                microphone: false,
                geolocation: false,
                clipboardWrite: true,
              },
            },
          },
        ],
      }),
    )
    const gateway = mcpServersGateway({
      connected: () => Promise.resolve(client([], { inspect }).value),
      after: () => () => {},
    })
    expect(await gateway.inspect("charts")).toEqual({
      ok: true,
      value: {
        complete: true,
        tools: [
          {
            name: "show_chart",
            readOnly: true,
            ui: {
              uri: "ui://c",
              csp: [{ name: "connect", origins: ["https://a"] }],
              permissions: ["clipboardWrite"],
            },
          },
        ],
      },
    })
  })

  it("names every CSP list and permission the protocol has, in the table's order", async () => {
    const inspect = vi.fn(() =>
      Promise.resolve({
        complete: true,
        tools: [
          {
            name: "all",
            ui: {
              uri: "ui://all",
              csp: {
                connectDomains: ["https://c"],
                resourceDomains: ["https://r"],
                frameDomains: ["https://f"],
                baseUriDomains: ["https://b"],
              },
              permissions: {
                camera: true,
                microphone: true,
                geolocation: true,
                clipboardWrite: true,
              },
            },
          },
        ],
      }),
    )
    const gateway = mcpServersGateway({
      connected: () => Promise.resolve(client([], { inspect }).value),
      after: () => () => {},
    })
    const answered = await gateway.inspect("all")
    if (!answered.ok) throw new Error("refused")
    expect(answered.value.tools[0].ui).toEqual({
      uri: "ui://all",
      csp: [
        { name: "connect", origins: ["https://c"] },
        { name: "resource", origins: ["https://r"] },
        { name: "frame", origins: ["https://f"] },
        { name: "base-uri", origins: ["https://b"] },
      ],
      permissions: ["camera", "microphone", "geolocation", "clipboardWrite"],
    })
  })
})
