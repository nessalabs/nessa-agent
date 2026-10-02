import {
  ConversationErrorCode,
  MAX_MCP_ARGUMENTS_BYTES,
  mcpAppDeadlines,
  NessaMcpAppError,
  NessaMcpResourceError,
  NessaRpcError,
  type McpAppsApi,
  type McpReadResourceResult,
  type McpResourceFailureCode,
} from "@nessa/client"
import { afterEach, describe, expect, it, vi } from "vitest"
import { uiResource } from "../../model/resource"
import type { ServerAddress } from "../../application/ports"
import { gatewayMcpAppServer } from "./mcp-app-server"

const conversationId = "6f0c2f4e-8a1b-4c3d-9e2f-0a1b2c3d4e5f"
const app = {
  executionId: "run",
  toolId: "call-1",
  instanceId: "0b8f3c2a-1d4e-4f5a-8b6c-7d8e9f0a1b2c",
}
const address: ServerAddress = { sessionId: conversationId, server: "charts", app }
const uri = "ui://charts/view"
const html = "<!doctype html><p>Chart — ünïcode</p>"
const bytes = new TextEncoder().encode(html)

const described: McpReadResourceResult = {
  uri,
  mimeType: "text/html;profile=mcp-app",
  size: bytes.byteLength,
  sha256: "a".repeat(64),
  ticket: "t".repeat(43),
  expiresInMs: 60000,
  csp: {
    connectDomains: ["https://api.example"],
    resourceDomains: ["https://cdn.example"],
    frameDomains: [],
    baseUriDomains: [],
  },
  permissions: {
    camera: false,
    microphone: false,
    geolocation: false,
    clipboardWrite: true,
  },
}

/** A client's `mcpApps` whose every answer the test gives. */
function fakeApps(overrides: Partial<McpAppsApi> = {}) {
  const apps = {
    callTool: vi.fn<McpAppsApi["callTool"]>(async () => ({
      resultJson: '{"content":[]}',
    })),
    readResource: vi.fn<McpAppsApi["readResource"]>(async () => described),
    fetchResource: vi.fn<McpAppsApi["fetchResource"]>(async () => bytes.slice()),
    releaseApp: vi.fn<McpAppsApi["releaseApp"]>(async () => ({
      requestId: "release",
      applied: true,
    })),
    ...overrides,
  }
  return apps
}

/** The client's error for a call the gateway refused with `code`, or failed without one. */
function refusedWith(code: string | undefined): NessaMcpAppError {
  const cause =
    code === undefined ? new Error("socket closed") : new NessaRpcError(code, "refused")
  return new NessaMcpAppError(conversationId, "request", app, cause)
}

afterEach(() => {
  vi.restoreAllMocks()
})

describe("an app's tool call over the gateway", () => {
  it("sends the app's call as the client's, and answers the server's result", async () => {
    const apps = fakeApps()
    const answer = await gatewayMcpAppServer(apps).callTool(address, "delete_rows", {
      rows: [1, 2],
    })
    expect(apps.callTool).toHaveBeenCalledExactlyOnceWith(
      conversationId,
      app,
      "charts",
      "delete_rows",
      '{"rows":[1,2]}',
    )
    expect(answer).toEqual({ kind: "ok", result: { content: [] } })
  })

  it("answers an error result as a result: it is the app's, not a refusal", async () => {
    const resultJson = '{"content":[{"type":"text","text":"no"}],"isError":true}'
    const apps = fakeApps({ callTool: vi.fn(async () => ({ resultJson })) })
    expect(await gatewayMcpAppServer(apps).callTool(address, "t", {})).toEqual({
      kind: "ok",
      result: JSON.parse(resultJson),
    })
  })

  it.each([
    [ConversationErrorCode.McpApprovalDenied, "The person declined this action"],
    [ConversationErrorCode.McpApprovalExpired, "No one answered the request in time"],
    [ConversationErrorCode.McpCancelled, "The request was cancelled"],
    [ConversationErrorCode.McpToolNotForApp, "This app may not use that tool"],
    [ConversationErrorCode.McpServerMismatch, "This app may not use that tool"],
    [ConversationErrorCode.McpAppUnknown, "This app may not use that tool"],
    [ConversationErrorCode.McpRequestTooLarge, "The request is too large"],
    [ConversationErrorCode.TemporarilyUnavailable, "Too many requests"],
  ])("answers %s as refused, in words", async (code, reason) => {
    const apps = fakeApps({ callTool: vi.fn(() => Promise.reject(refusedWith(code))) })
    expect(await gatewayMcpAppServer(apps).callTool(address, "t", {})).toEqual({
      kind: "refused",
      reason,
    })
  })

  it("answers a conversation with no session of the server as the server gone", async () => {
    const apps = fakeApps({
      callTool: vi.fn(() =>
        Promise.reject(refusedWith(ConversationErrorCode.McpSessionUnavailable)),
      ),
    })
    expect(await gatewayMcpAppServer(apps).callTool(address, "t", {})).toEqual({
      kind: "server-gone",
    })
  })

  it.each([
    ConversationErrorCode.McpTimedOut,
    ConversationErrorCode.McpRemoteError,
    ConversationErrorCode.McpResultTooLarge,
    ConversationErrorCode.ConversationNotFound,
    undefined,
  ])("answers %s as failed", async (code) => {
    const apps = fakeApps({ callTool: vi.fn(() => Promise.reject(refusedWith(code))) })
    expect(await gatewayMcpAppServer(apps).callTool(address, "t", {})).toEqual({
      kind: "failed",
    })
  })

  it("answers a result that is not one JSON object, or past the bounds an app's message is read under, as failed", async () => {
    for (const resultJson of [
      "[]",
      "3",
      "{",
      `${'{"a":'.repeat(65)}1${"}".repeat(65)}`,
    ]) {
      const apps = fakeApps({ callTool: vi.fn(async () => ({ resultJson })) })
      expect(await gatewayMcpAppServer(apps).callTool(address, "t", {})).toEqual({
        kind: "failed",
      })
    }
  })

  it("refuses arguments past the most a review shows, before anything is sent", async () => {
    // `{"a":"…"}` is 8 bytes around the string.
    const longest = { a: "x".repeat(MAX_MCP_ARGUMENTS_BYTES - 8) }
    const over = { a: "x".repeat(MAX_MCP_ARGUMENTS_BYTES - 7) }
    const apps = fakeApps()
    const server = gatewayMcpAppServer(apps)
    expect(await server.callTool(address, "t", over)).toEqual({
      kind: "refused",
      reason: "The request is too large",
    })
    expect(apps.callTool).not.toHaveBeenCalled()
    expect(await server.callTool(address, "t", longest)).toEqual({
      kind: "ok",
      result: { content: [] },
    })
    expect(apps.callTool).toHaveBeenCalledOnce()
  })

  it("rejects for a fault that is no answer of the gateway's, for the bridge to log", async () => {
    const fault = new TypeError("Conversation ID must be a canonical lowercase UUID")
    const apps = fakeApps({ callTool: vi.fn(() => Promise.reject(fault)) })
    await expect(gatewayMcpAppServer(apps).callTool(address, "t", {})).rejects.toBe(fault)
  })
})

describe("an app's resource over the gateway", () => {
  it("reads, redeems the ticket against what was described, and answers the app's HTML", async () => {
    const apps = fakeApps({
      readResource: vi.fn(async () => ({
        ...described,
        domain: "charts.example",
        prefersBorder: true,
      })),
    })
    const answer = await gatewayMcpAppServer(apps).readResource(address, uri)
    expect(apps.readResource).toHaveBeenCalledExactlyOnceWith(
      conversationId,
      app,
      "charts",
      uri,
    )
    expect(apps.fetchResource).toHaveBeenCalledExactlyOnceWith(
      described.ticket,
      expect.objectContaining({ size: described.size, sha256: described.sha256 }),
    )
    expect(answer.kind).toBe("ok")
    if (answer.kind !== "ok") return
    expect(answer.result).toEqual({
      contents: [
        {
          uri,
          mimeType: "text/html;profile=mcp-app",
          text: html,
          _meta: {
            ui: {
              csp: described.csp,
              permissions: described.permissions,
              domain: "charts.example",
              prefersBorder: true,
            },
          },
        },
      ],
    })
    // What the bridge reads of it: the HTML, and the CSP the app asked for.
    const resource = uiResource(answer.result, uri)
    expect(resource?.html).toBe(html)
    expect(resource?.csp.connect.map((source) => source.host)).toEqual(["api.example"])
  })

  it("leaves out what the app did not say", async () => {
    const answer = await gatewayMcpAppServer(fakeApps()).readResource(address, uri)
    if (answer.kind !== "ok") throw new Error(answer.kind)
    const ui = (answer.result.contents as never)[0]["_meta"]["ui"]
    expect(Object.keys(ui)).toEqual(["csp", "permissions"])
  })

  it.each<McpResourceFailureCode>([
    "not_found",
    "unavailable",
    "integrity",
    "aborted",
    "timeout",
    "unreachable",
    "unexpected_response",
  ])("answers a ticket that came to %s as failed", async (code) => {
    const apps = fakeApps({
      fetchResource: vi.fn(() => Promise.reject(new NessaMcpResourceError(code))),
    })
    expect(await gatewayMcpAppServer(apps).readResource(address, uri)).toEqual({
      kind: "failed",
    })
  })

  it("answers bytes that are not UTF-8 as failed", async () => {
    const apps = fakeApps({
      fetchResource: vi.fn(async () => new Uint8Array([0xff, 0xfe])),
    })
    expect(await gatewayMcpAppServer(apps).readResource(address, uri)).toEqual({
      kind: "failed",
    })
  })

  it("answers the read's refusals as a call's, in a resource's words", async () => {
    const answer = async (code: string) => {
      const apps = fakeApps({
        readResource: vi.fn(() => Promise.reject(refusedWith(code))),
      })
      const read = await gatewayMcpAppServer(apps).readResource(address, uri)
      expect(apps.fetchResource).not.toHaveBeenCalled()
      return read
    }
    expect(await answer(ConversationErrorCode.McpAppUnknown)).toEqual({
      kind: "refused",
      reason: "This app may not use that resource",
    })
    expect(await answer(ConversationErrorCode.McpSessionUnavailable)).toEqual({
      kind: "server-gone",
    })
    expect(await answer(ConversationErrorCode.McpTimedOut)).toEqual({ kind: "failed" })
  })
})

describe("the mount's release and the server's time", () => {
  it("releases the mount as the client's app", () => {
    const apps = fakeApps()
    gatewayMcpAppServer(apps).release(address)
    expect(apps.releaseApp).toHaveBeenCalledExactlyOnceWith(conversationId, app)
  })

  it("says so when a release was not acknowledged, and throws nothing", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const refused = new Error("socket closed")
    const apps = fakeApps({ releaseApp: vi.fn(() => Promise.reject(refused)) })
    expect(() => gatewayMcpAppServer(apps).release(address)).not.toThrow()
    await vi.waitFor(() => expect(warn).toHaveBeenCalledWith(expect.any(String), refused))
  })

  it("waits as long as the client does: the review, the call, the read and the ticket", () => {
    const { answersWithin } = gatewayMcpAppServer(fakeApps())
    expect(answersWithin).toEqual({
      callTool: mcpAppDeadlines.callToolMs,
      readResource: mcpAppDeadlines.readResourceMs + mcpAppDeadlines.fetchResourceMs,
    })
  })
})
