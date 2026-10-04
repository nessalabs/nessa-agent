import { describe, expect, it, vi } from "vitest"

import { NessaMcpServersError } from "../application/mcp-servers-error.js"
import { NessaRpcError } from "../application/rpc-error.js"
import type { RequestDeadline } from "../application/session-port.js"
import { McpServersErrorCode, mcpServerInspect } from "../generated/product.js"
import { createMcpServersApi, mayManageMcpServers } from "./mcp-servers-api.js"

const entry = {
  kind: "stdio",
  name: "charts",
  command: "/usr/bin/node",
  args: ["server.mjs"],
  envNames: ["TOKEN"],
  enabled: true,
  managed: false,
}
const managed = { ...entry, name: "nessa", envNames: [], managed: true }
const csp = {
  connectDomains: [],
  resourceDomains: [],
  frameDomains: [],
  baseUriDomains: [],
}
const permissions = {
  camera: false,
  microphone: false,
  geolocation: false,
  clipboardWrite: false,
}

function session(answer: (method: string, params: unknown) => unknown) {
  const calls: { method: string; params: unknown; deadline?: RequestDeadline }[] = []
  const request = vi.fn(
    async (method: string, params: unknown, deadline?: RequestDeadline) => {
      calls.push({ method, params, ...(deadline ? { deadline } : {}) })
      return answer(method, params)
    },
  )
  return { api: createMcpServersApi({ request }), calls }
}

const refusing = (code: string, details?: unknown) =>
  session(() => {
    throw new NessaRpcError(code, "refused", details)
  })

async function failure(run: Promise<unknown>): Promise<NessaMcpServersError> {
  const error = await run.then(
    () => undefined,
    (error: unknown) => error,
  )
  expect(error).toBeInstanceOf(NessaMcpServersError)
  return error as NessaMcpServersError
}

describe("client.mcpServers answers", () => {
  it("lists the servers in the gateway's order", async () => {
    const { api, calls } = session(() => ({
      revision: "r1",
      servers: [entry, managed],
    }))
    const listed = await api.list()
    expect(calls).toEqual([{ method: "mcpServers.list", params: {} }])
    expect(listed.revision).toBe("r1")
    expect(listed.servers.map((each) => each.name)).toEqual(["charts", "nessa"])
  })

  it("sends a save and a remove as given, and answers the new revision", async () => {
    const { api, calls } = session(() => ({ revision: "r2" }))
    const save = {
      revision: "r1",
      previousName: "old",
      server: { ...entry, env: [{ name: "TOKEN", value: null }] },
    }
    delete (save.server as Partial<typeof entry>).envNames
    delete (save.server as Partial<typeof entry>).managed
    await expect(api.save(save as never)).resolves.toEqual({ revision: "r2" })
    await expect(api.remove({ revision: "r2", name: "charts" })).resolves.toEqual({
      revision: "r2",
    })
    expect(calls.map((each) => [each.method, each.params])).toEqual([
      ["mcpServers.save", save],
      ["mcpServers.remove", { revision: "r2", name: "charts" }],
    ])
  })

  it("waits the published inspection deadline, and reads a cut inspection", async () => {
    const { api, calls } = session(() => ({
      complete: false,
      cut: "ui",
      tools: [
        {
          name: "show_chart",
          readOnlyHint: true,
          ui: { uri: "ui://c", csp, permissions },
        },
        { name: "drop", destructiveHint: true },
      ],
    }))
    const inspected = await api.inspect("charts")
    expect(calls).toEqual([
      {
        method: "mcpServers.inspect",
        params: { name: "charts" },
        deadline: { atLeastMs: mcpServerInspect.requestDeadlineMs },
      },
    ])
    expect(inspected.cut).toBe("ui")
    expect(inspected.tools[0].ui?.uri).toBe("ui://c")
    expect(inspected.tools[1]).toEqual({ name: "drop", destructiveHint: true })
  })

  it.each([
    ["a list with unknown fields", "list", { revision: "r", servers: [], extra: 1 }],
    [
      "an entry of another kind",
      "list",
      { revision: "r", servers: [{ ...entry, kind: "http" }] },
    ],
    [
      "an entry without its managed flag",
      "list",
      { revision: "r", servers: [{ ...entry, managed: undefined }] },
    ],
    ["a write without a revision", "remove", {}],
    ["an incomplete inspection without a cut", "inspect", { complete: false, tools: [] }],
    [
      "a complete inspection with a cut",
      "inspect",
      { complete: true, cut: "ui", tools: [] },
    ],
    [
      "a hint that is not a boolean",
      "inspect",
      { complete: true, tools: [{ name: "t", readOnlyHint: "yes" }] },
    ],
  ])("refuses %s as no answer", async (_, method, answer) => {
    const { api } = session(() => answer)
    const run =
      method === "list"
        ? api.list()
        : method === "remove"
          ? api.remove({ revision: "r", name: "n" })
          : api.inspect("n")
    const error = await failure(run)
    expect(error.refusal).toBeUndefined()
    expect(error.forbidden).toBe(false)
  })
})

describe("NessaMcpServersError narrows the refusal", () => {
  it("types the invalid details by problem and name", async () => {
    const { api } = refusing("mcp_servers_invalid", { problem: "command" })
    const error = await failure(api.list())
    expect(error.method).toBe("mcpServers.list")
    expect(error.refusal).toEqual({
      code: "mcp_servers_invalid",
      details: { problem: "command" },
    })
    expect(error.code).toBe("mcp_servers_invalid")
  })

  it("types the invalid details' server and variable", async () => {
    const { api } = refusing("mcp_servers_invalid", {
      problem: "environment_value",
      server: "charts",
      name: "TOKEN",
    })
    expect((await failure(api.list())).refusal).toEqual({
      code: "mcp_servers_invalid",
      details: { problem: "environment_value", server: "charts", name: "TOKEN" },
    })
  })

  it("narrows mcp_servers_stopping, which carries no details", async () => {
    const { api } = refusing("mcp_servers_stopping")
    const error = await failure(api.inspect("n"))
    expect(error.refusal).toEqual({ code: "mcp_servers_stopping" })
    expect(error.code).toBe(McpServersErrorCode.McpServersStopping)
  })

  it("keeps the refusal and drops details that are not its code's shape", async () => {
    const { api } = refusing("mcp_servers_invalid", { problem: "constructor" })
    const error = await failure(api.list())
    expect(error.refusal).toEqual({ code: "mcp_servers_invalid", details: undefined })
  })

  it("types a conflict's revision", async () => {
    const { api } = refusing("mcp_servers_revision_conflict", { revision: "r9" })
    expect((await failure(api.remove({ revision: "r1", name: "n" }))).refusal).toEqual({
      code: "mcp_servers_revision_conflict",
      details: { revision: "r9" },
    })
  })

  it("types an audit refusal's applied and code, never audit_unavailable", async () => {
    const applied = refusing("audit_unavailable", {
      applied: true,
      code: "mcp_servers_busy",
    })
    expect((await failure(applied.api.list())).refusal).toEqual({
      code: "audit_unavailable",
      details: { applied: true, code: "mcp_servers_busy" },
    })
    const looped = refusing("audit_unavailable", {
      applied: false,
      code: "audit_unavailable",
    })
    expect((await failure(looped.api.list())).refusal).toEqual({
      code: "audit_unavailable",
      details: { applied: false },
    })
  })

  it("types a remote error's code and message", async () => {
    const { api } = refusing("mcp_server_remote_error", { code: -32601, message: "no" })
    expect((await failure(api.inspect("n"))).refusal).toEqual({
      code: "mcp_server_remote_error",
      details: { code: -32601, message: "no" },
    })
  })

  it.each(["constructor", "toString", "__proto__", "conversation_not_found", "unknown"])(
    "gives no typed refusal for %s",
    async (code) => {
      const error = await failure(refusing(code).api.list())
      expect(error.refusal).toBeUndefined()
      expect(error.code).toBeUndefined()
      expect(error.forbidden).toBe(false)
    },
  )

  it("says forbidden for the session's refusal of the caller", async () => {
    const error = await failure(refusing("forbidden").api.list())
    expect(error.forbidden).toBe(true)
    expect(error.refusal).toBeUndefined()
  })

  it("keeps a transport failure as its cause", async () => {
    const lost = new Error("socket closed")
    const { api } = session(() => {
      throw lost
    })
    const error = await failure(api.save({} as never))
    expect(error.cause).toBe(lost)
    expect(error.refusal).toBeUndefined()
  })
})

describe("mayManageMcpServers", () => {
  const grant = (action: string) => ({
    action,
    resource: { organizationId: "o", id: "r" },
  })
  it("is true only with the grant the gateway asks for", () => {
    expect(mayManageMcpServers({ grants: [grant("credential.manage")] })).toBe(true)
    expect(
      mayManageMcpServers({
        grants: [grant("conversation.read"), grant("conversation.write")],
      }),
    ).toBe(false)
    expect(mayManageMcpServers({ grants: [] })).toBe(false)
  })
})
