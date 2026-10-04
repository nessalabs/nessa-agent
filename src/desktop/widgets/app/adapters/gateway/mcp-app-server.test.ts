/**
 * `McpAppServer` over a fake `client.mcpApps`, and, in the last group, the
 * real bridge over it. A test's name begins with its rows (#384's or #349's
 * state table) and marks an input the real client or gateway does not produce.
 */
import {
  ConversationErrorCode,
  MAX_MCP_ARGUMENTS_BYTES,
  MAX_MCP_RESOURCE_BYTES,
  mcpAppDeadlines,
  mcpAppRequestProblem,
  NessaConversationControlError,
  NessaMcpAppError,
  NessaMcpResourceError,
  NessaRequestTooLargeError,
  NessaRpcError,
  type McpAppReference,
  type McpAppsApi,
  type McpReadResourceResult,
  type McpResourceFailureCode,
} from "@nessa/client"
import { inspect } from "node:util"
import { afterEach, describe, expect, it, vi } from "vitest"
import { createAppBridge } from "../../application/bridge"
import type { AppAddress, McpAppPorts, ServerAnswer } from "../../application/ports"
import type { AppViewState } from "../../model/app-view"
import { readEnvelope, type Outgoing } from "../../model/json-rpc"
import { readFromFrame } from "../../model/messages"
import { uiResource } from "../../model/resource"
import { fixtureCall } from "../../fixture/fixture-plugin"
import { gatewayAppServer as serverOver, releaseRetryMs } from "./mcp-app-server"

/** The waits a server's releases asked for; each runs at once. */
let waited: number[] = []
const immediately = (ms: number, run: () => void) => {
  waited.push(ms)
  run()
  return () => {}
}
/**
 * The adapter, on a clock that runs every wait at once and records it, minting
 * request ids `release-1`, `release-2`, ... in order, one count per server.
 */
const gatewayAppServer = (apps: McpAppsApi) => {
  let minted = 0
  return serverOver(apps, immediately, () => `release-${++minted}`)
}

const conversationId = "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d"
const app: McpAppReference = {
  executionId: "execution-1",
  toolId: "call-1",
  instanceId: "6f1d2c3b-4a5e-4f60-8172-839405a6b7c8",
}
const address: AppAddress = { sessionId: conversationId, server: "weather", app }

/** A mount's signal while it is not released. */
const live = () => new AbortController().signal
const uri = "ui://weather/app.html"
const ticket = "T".repeat(43)
const page = "<!doctype html><p>Weather</p>"
const pageBytes = new TextEncoder().encode(page)

const described: McpReadResourceResult = {
  uri,
  mimeType: "text/html;profile=mcp-app",
  size: pageBytes.byteLength,
  sha256: "a".repeat(64),
  ticket,
  expiresInMs: 60000,
  csp: {
    connectDomains: ["https://api.weather.example"],
    resourceDomains: [],
    frameDomains: [],
    baseUriDomains: [],
  },
  permissions: {
    camera: false,
    microphone: false,
    geolocation: false,
    clipboardWrite: false,
  },
}

/** The error `client.mcpApps` throws when the gateway refused with `code`. */
function refusal(code: string, details?: unknown): NessaMcpAppError {
  return new NessaMcpAppError(
    conversationId,
    "request-1",
    app,
    new NessaRpcError(code, `refused: ${code}`, details),
  )
}

/** A fake `client.mcpApps`: every method a spy, each answering as the test says. */
function fakeApps(overrides: Partial<McpAppsApi> = {}) {
  const apps = {
    callTool: vi.fn<McpAppsApi["callTool"]>(async () => ({
      resultJson: '{"content":[{"type":"text","text":"72"}]}',
    })),
    readResource: vi.fn<McpAppsApi["readResource"]>(async () => described),
    fetchResource: vi.fn<McpAppsApi["fetchResource"]>(
      async () => new Uint8Array(pageBytes) as Uint8Array<ArrayBuffer>,
    ),
    releaseApp: vi.fn<McpAppsApi["releaseApp"]>(async () => ({
      requestId: "request-1",
      applied: true,
    })),
  }
  return Object.assign(apps, overrides)
}

afterEach(() => {
  vi.restoreAllMocks()
})

describe("tools/call", () => {
  it("A1: the server's result is the app's, an error result included, from the view's own address", async () => {
    const apps = fakeApps()
    const server = gatewayAppServer(apps)
    expect(await server.callTool(address, "get_weather", { city: "Oslo" })).toEqual({
      kind: "ok",
      result: { content: [{ type: "text", text: "72" }] },
    })
    expect(apps.callTool).toHaveBeenCalledWith(
      conversationId,
      app,
      "weather",
      "get_weather",
      '{"city":"Oslo"}',
    )
    apps.callTool.mockResolvedValueOnce({ resultJson: '{"content":[],"isError":true}' })
    expect(await server.callTool(address, "t", {})).toEqual({
      kind: "ok",
      result: { content: [], isError: true },
    })
  })

  it("A2: arguments past the client's published bound, in UTF-8 bytes, are refused before anything is sent", async () => {
    const apps = fakeApps()
    const server = gatewayAppServer(apps)
    // `{"a":"…"}` is 8 bytes around the text; "é" is two bytes in one character.
    const fits = { a: "é".repeat((MAX_MCP_ARGUMENTS_BYTES - 8) / 2) }
    expect(new TextEncoder().encode(JSON.stringify(fits)).byteLength).toBe(
      MAX_MCP_ARGUMENTS_BYTES,
    )
    expect((await server.callTool(address, "t", fits)).kind).toBe("ok")
    expect(await server.callTool(address, "t", { a: `${fits.a}x` })).toEqual({
      kind: "refused",
      reason: `Arguments must contain at most ${MAX_MCP_ARGUMENTS_BYTES} UTF-8 bytes`,
    })
    expect(apps.callTool).toHaveBeenCalledTimes(1)
  })

  it("A2: a tool name or resource URI past the client's bounds is the app's request refused, before sending, and not logged", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const apps = fakeApps()
    const server = gatewayAppServer(apps)
    // `é` is two UTF-8 bytes: the bounds are bytes, not characters.
    expect((await server.callTool(address, "é".repeat(64), {})).kind).toBe("ok")
    expect(await server.callTool(address, `${"é".repeat(64)}x`, {})).toEqual({
      kind: "refused",
      reason: mcpAppRequestProblem.tool(`${"é".repeat(64)}x`),
    })
    expect(await server.callTool(address, "", {})).toMatchObject({ kind: "refused" })
    // A lone surrogate is no Unicode: the gateway could not decode the frame.
    for (const answer of [
      await server.callTool(address, "get\ud800", {}),
      await server.readResource(address, "ui://w/\ud800", live()),
    ])
      expect(answer).toMatchObject({ kind: "refused" })
    const long = `ui://w/${"a".repeat(2048)}`
    expect(await server.readResource(address, long, live())).toEqual({
      kind: "refused",
      reason: mcpAppRequestProblem.uri(long),
    })
    expect(apps.callTool).toHaveBeenCalledTimes(1)
    expect(apps.readResource).not.toHaveBeenCalled()
    expect(error).not.toHaveBeenCalled()
  })

  it("A13: arguments the gateway judges invalid — a lone surrogate inside them, escaped — are the gateway's refusal", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const apps = fakeApps({
      callTool: vi.fn(() =>
        Promise.reject(refusal(ConversationErrorCode.InvalidRequest)),
      ),
    })
    // Escaped in the arguments' text, it reaches the gateway, which answers.
    expect(
      await gatewayAppServer(apps).callTool(address, "get", { a: "\udc00" }),
    ).toEqual({
      kind: "refused",
      reason: "The gateway refused the request as invalid",
    })
    expect(apps.callTool).toHaveBeenCalledWith(
      conversationId,
      app,
      "weather",
      "get",
      JSON.stringify({ a: "\udc00" }),
    )
    expect(error).not.toHaveBeenCalled()
  })

  it("A14: a request too large for the gateway to take is refused, nothing sent, nothing logged (on a read, an unreachable case pinned as a defence)", async () => {
    // Unreachable for a read: its frame (bounded name, URI and ids) is far
    // below the gateway's request frame limit, so the client never throws
    // this for one. The read half pins what the adapter would answer anyway.
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const tooLarge = () =>
      Promise.reject(
        new NessaMcpAppError(
          conversationId,
          "r",
          app,
          new NessaRequestTooLargeError("mcp.callTool", 70_000),
        ),
      )
    const server = gatewayAppServer(
      fakeApps({ callTool: vi.fn(tooLarge), readResource: vi.fn(tooLarge) }),
    )
    const refused = {
      kind: "refused",
      reason: "The request is larger than the gateway accepts",
    }
    expect(await server.callTool(address, "t", { a: '"'.repeat(16380) })).toEqual(refused)
    expect(await server.readResource(address, uri, live())).toEqual(refused)
    expect(error).not.toHaveBeenCalled()
  })

  it("A11: any other TypeError the client throws is a fault of the host's, logged — never the app's refusal", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const thrown = () =>
      Promise.reject(new TypeError("Conversation ID must be a canonical lowercase UUID"))
    const server = gatewayAppServer(
      fakeApps({ callTool: vi.fn(thrown), readResource: vi.fn(thrown) }),
    )
    expect(await server.callTool(address, "t", {})).toEqual({ kind: "failed" })
    expect(await server.readResource(address, uri, live())).toEqual({ kind: "failed" })
    expect(error).toHaveBeenCalledTimes(2)
  })

  it.each([
    [ConversationErrorCode.McpApprovalDenied, "The person declined this action"],
    [ConversationErrorCode.McpApprovalExpired, "No one answered in time"],
    [ConversationErrorCode.McpCancelled, "The request was withdrawn"],
    [ConversationErrorCode.McpToolNotForApp, "This app may not use that tool"],
    [ConversationErrorCode.McpServerMismatch, "This app may not use that tool"],
    [ConversationErrorCode.McpAppUnknown, "This app may not use that tool"],
    [
      ConversationErrorCode.McpRequestTooLarge,
      "The request is larger than the gateway accepts",
    ],
  ])("A3–A7: %s is refused with the gateway's reason", async (code, reason) => {
    const server = gatewayAppServer(
      fakeApps({ callTool: vi.fn(() => Promise.reject(refusal(code))) }),
    )
    expect(await server.callTool(address, "t", {})).toEqual({ kind: "refused", reason })
  })

  it("A8, L1b, R5b-4: no room on the app lane is busy, for the same request to be made again", async () => {
    const busy = () =>
      Promise.reject(refusal(ConversationErrorCode.TemporarilyUnavailable))
    const server = gatewayAppServer(
      fakeApps({ callTool: vi.fn(busy), readResource: vi.fn(busy) }),
    )
    expect(await server.callTool(address, "t", {})).toEqual({ kind: "busy" })
    expect(await server.readResource(address, uri, live())).toEqual({ kind: "busy" })
  })

  it.each([
    ConversationErrorCode.McpSessionUnavailable,
    ConversationErrorCode.ConversationNotFound,
    ConversationErrorCode.ConversationDeleted,
    ConversationErrorCode.ConversationClosed,
  ])("A9: %s is the server gone, not a failure", async (code) => {
    const server = gatewayAppServer(
      fakeApps({ callTool: vi.fn(() => Promise.reject(refusal(code))) }),
    )
    expect(await server.callTool(address, "t", {})).toEqual({ kind: "server-gone" })
  })

  it.each([
    [-32002, "Resource not found"],
    [-1, "negative"],
    [7, "positive"],
  ])(
    "A10: the server's own JSON-RPC error %i is passed on as it came",
    async (code, message) => {
      const server = gatewayAppServer(
        fakeApps({
          callTool: vi.fn(() =>
            Promise.reject(
              refusal(ConversationErrorCode.McpRemoteError, { code, message }),
            ),
          ),
        }),
      )
      expect(await server.callTool(address, "t", {})).toEqual({
        kind: "failed",
        error: { code, message },
      })
    },
  )

  it("A11: every other answer is a failure, server-gone and refused kept apart from it (inputs the real client or gateway does not produce included: a fractional remote code, a prototype-key code, a stray string)", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const thrown: unknown[] = [
      // mcp_remote_error without details: an answer that was no MCP answer.
      refusal(ConversationErrorCode.McpRemoteError),
      // Unreachable: the gateway's remote code is a range-checked integer.
      refusal(ConversationErrorCode.McpRemoteError, { code: 1.5, message: "x" }),
      refusal(ConversationErrorCode.McpTimedOut),
      refusal(ConversationErrorCode.McpResultTooLarge),
      refusal(ConversationErrorCode.AuditUnavailable),
      // A code this build does not know, and one that names a prototype's
      // key (unreachable: no ConversationErrorCode).
      refusal("mcp_something_new"),
      refusal("constructor"),
      // No answer at all, and a robustness input outside what the client's
      // `call()` (`mcp-apps-api.ts`) is written to throw.
      new NessaMcpAppError(conversationId, "r", app, new Error("socket closed")),
      "a string",
    ]
    for (const each of thrown) {
      const server = gatewayAppServer(
        fakeApps({ callTool: vi.fn(() => Promise.reject(each)) }),
      )
      expect(await server.callTool(address, "t", {})).toEqual({ kind: "failed" })
    }
    // What is not the gateway's typed answer is logged: the two unknown codes,
    // no answer, and the stray value.
    expect(error).toHaveBeenCalledTimes(4)
  })

  // `outcomes` is a total table, read the same way for both methods. This
  // checks that totality over every code. Many are codes the gateway never
  // sends to a given method; for those, the input is unreachable.
  it.each(["callTool", "readResource"] as const)(
    "A3–A9, A11, R5b: outcomes is total over every ConversationErrorCode on %s, the same for both methods, including codes the gateway never sends to it (gate 11)",
    async (method) => {
      const error = vi.spyOn(console, "error").mockImplementation(() => {})
      const kinds = new Map<string, ServerAnswer["kind"]>()
      for (const code of Object.values(ConversationErrorCode)) {
        const refused = () => Promise.reject(refusal(code))
        const apps = fakeApps({ callTool: vi.fn(refused), readResource: vi.fn(refused) })
        const server = gatewayAppServer(apps)
        const answer =
          method === "callTool"
            ? await server.callTool(address, "t", {})
            : await server.readResource(address, uri, live())
        kinds.set(code, answer.kind)
        expect(apps.fetchResource).not.toHaveBeenCalled()
      }
      // Every code is placed: none is an unplaced fault, so none is logged.
      expect(error).not.toHaveBeenCalled()
      const refused = [...kinds].filter(([, kind]) => kind === "refused").map(([c]) => c)
      const gone = [...kinds].filter(([, kind]) => kind === "server-gone").map(([c]) => c)
      const busy = [...kinds].filter(([, kind]) => kind === "busy").map(([c]) => c)
      expect(busy).toEqual(["temporarily_unavailable"])
      expect(refused.sort()).toEqual(
        [
          "mcp_app_unknown",
          "mcp_approval_denied",
          "mcp_approval_expired",
          "mcp_cancelled",
          "mcp_request_too_large",
          "mcp_server_mismatch",
          "mcp_tool_not_for_app",
          "invalid_request",
        ].sort(),
      )
      expect(gone.sort()).toEqual(
        [
          "conversation_closed",
          "conversation_deleted",
          "conversation_not_found",
          "mcp_session_unavailable",
        ].sort(),
      )
      // Everything else — audit, capacity, the agent's own refusals — fails.
      const placed = new Set([...refused, ...gone, ...busy])
      for (const [code, kind] of kinds)
        if (!placed.has(code)) expect([code, kind]).toEqual([code, "failed"])
      expect(kinds.size).toBe(Object.values(ConversationErrorCode).length)
    },
  )

  it("D1: a tools/call is waited for as long as the client waits for a review and the call", () => {
    expect(gatewayAppServer(fakeApps()).callWithin).toBe(mcpAppDeadlines.callToolMs)
  })
})

describe("resources/read and the ticket", () => {
  it("R1, R2: reads, redeems the ticket once with what was described, and hands back the app — never the ticket", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const apps = fakeApps()
    const answer = await gatewayAppServer(apps).readResource(address, uri, live())
    expect(apps.readResource).toHaveBeenCalledWith(conversationId, app, "weather", uri)
    expect(apps.fetchResource).toHaveBeenCalledTimes(1)
    expect(apps.fetchResource).toHaveBeenCalledWith(
      ticket,
      { size: described.size, sha256: described.sha256 },
      { signal: expect.any(AbortSignal) },
    )
    expect(answer.kind).toBe("ok")
    if (answer.kind !== "ok") return
    expect(JSON.stringify(answer.result)).not.toContain(ticket)
    expect(error).not.toHaveBeenCalled()
    // The bridge's own reader takes it: the HTML, and the CSP the gateway passed on.
    expect(uiResource(answer.result, uri)).toMatchObject({
      html: page,
      csp: { connect: [{ scheme: "https", host: "api.weather.example" }] },
    })
  })

  it("R2: the domain and border the app asked for travel with it when it asked", async () => {
    const apps = fakeApps({
      readResource: vi.fn(async () => ({
        ...described,
        domain: "weather.example",
        prefersBorder: false,
      })),
    })
    const answer = await gatewayAppServer(apps).readResource(address, uri, live())
    expect(answer).toMatchObject({
      kind: "ok",
      result: {
        contents: [
          { _meta: { ui: { domain: "weather.example", prefersBorder: false } } },
        ],
      },
    })
  })

  it("R3: bytes that are not UTF-8 are no app", async () => {
    const apps = fakeApps({
      fetchResource: vi.fn(
        async () => new Uint8Array([0xff, 0xfe]) as Uint8Array<ArrayBuffer>,
      ),
    })
    expect(await gatewayAppServer(apps).readResource(address, uri, live())).toEqual({
      kind: "failed",
    })
  })

  // Each as the client builds it (`mcp-apps-api.ts` `fetchResource`): the
  // route's status when it answered, the transport's error as the cause when
  // the transport failed, and neither for a deadline that passed.
  it.each<[McpResourceFailureCode, () => NessaMcpResourceError]>([
    ["not_found", () => new NessaMcpResourceError("not_found", 404)],
    ["unavailable", () => new NessaMcpResourceError("unavailable", 503)],
    ["integrity", () => new NessaMcpResourceError("integrity", 200)],
    ["timeout", () => new NessaMcpResourceError("timeout")],
    [
      "unreachable",
      () =>
        new NessaMcpResourceError(
          "unreachable",
          undefined,
          new TypeError("fetch failed"),
        ),
    ],
    ["unexpected_response", () => new NessaMcpResourceError("unexpected_response", 500)],
  ])(
    "R4: a redemption that fails as %s is a failure, never retried, and its log holds no ticket",
    async (_code, thrown) => {
      const error = vi.spyOn(console, "error").mockImplementation(() => {})
      const apps = fakeApps({ fetchResource: vi.fn(() => Promise.reject(thrown())) })
      expect(await gatewayAppServer(apps).readResource(address, uri, live())).toEqual({
        kind: "failed",
      })
      expect(apps.fetchResource).toHaveBeenCalledTimes(1)
      expect(error).toHaveBeenCalledTimes(1)
      // Everything logged, every argument and its causes, holds no ticket.
      expect(inspect(error.mock.calls, { depth: 10 })).not.toContain(ticket)
    },
  )

  it("R5, R5b-1, R5b-6: a read the gateway did not answer is the A table's answer — refused in a resource's words, the server gone, or failed; no ticket is redeemed", async () => {
    const cases: [string, ServerAnswer][] = [
      [
        ConversationErrorCode.McpAppUnknown,
        { kind: "refused", reason: "This app may not read that resource" },
      ],
      [
        ConversationErrorCode.McpServerMismatch,
        { kind: "refused", reason: "This app may not read that resource" },
      ],
      [ConversationErrorCode.McpSessionUnavailable, { kind: "server-gone" }],
      [ConversationErrorCode.McpTimedOut, { kind: "failed" }],
    ]
    for (const [code, expected] of cases) {
      const apps = fakeApps({ readResource: vi.fn(() => Promise.reject(refusal(code))) })
      expect(await gatewayAppServer(apps).readResource(address, uri, live())).toEqual(
        expected,
      )
      expect(apps.fetchResource).not.toHaveBeenCalled()
    }
  })

  // R5b-2, -3, -5, -6 and -7: each answers a read as it answers a call (one
  // `answerFor`), and these codes' words are the same for both.
  const remote = { code: -32002, message: "Resource not found" }
  it.each<[string, string, unknown, ServerAnswer]>([
    [
      "R5b-2",
      ConversationErrorCode.InvalidRequest,
      undefined,
      { kind: "refused", reason: "The gateway refused the request as invalid" },
    ],
    [
      "R5b-3",
      ConversationErrorCode.McpCancelled,
      undefined,
      { kind: "refused", reason: "The request was withdrawn" },
    ],
    ["R5b-5", ConversationErrorCode.McpResultTooLarge, undefined, { kind: "failed" }],
    [
      "R5b-6",
      ConversationErrorCode.McpRemoteError,
      remote,
      { kind: "failed", error: remote },
    ],
    ["R5b-6", ConversationErrorCode.McpRemoteError, undefined, { kind: "failed" }],
    [
      "R5b-7",
      ConversationErrorCode.ConversationNotFound,
      undefined,
      { kind: "server-gone" },
    ],
    [
      "R5b-7",
      ConversationErrorCode.ConversationDeleted,
      undefined,
      { kind: "server-gone" },
    ],
    [
      "R5b-7",
      ConversationErrorCode.ConversationClosed,
      undefined,
      { kind: "server-gone" },
    ],
  ])(
    "%s: a read refused %s (details %o) gets what a call refused so gets; nothing redeemed, nothing logged",
    async (_row, code, details, expected) => {
      const error = vi.spyOn(console, "error").mockImplementation(() => {})
      const refused = () => Promise.reject(refusal(code, details))
      const apps = fakeApps({ callTool: vi.fn(refused), readResource: vi.fn(refused) })
      const server = gatewayAppServer(apps)
      expect(await server.readResource(address, uri, live())).toEqual(expected)
      expect(await server.callTool(address, "t", {})).toEqual(expected)
      expect(apps.fetchResource).not.toHaveBeenCalled()
      expect(error).not.toHaveBeenCalled()
    },
  )

  // Unreachable through the real client: `readResource` already refuses an
  // answer whose ticket, size or SHA-256 is malformed (`mcpReadResourceResult`),
  // so `fetchResource` is never handed one. Pinned as a defence, as R8 is.
  it.each([
    "Resource ticket must be 43 base64url characters",
    `Resource size must be 0-${MAX_MCP_RESOURCE_BYTES} bytes`,
    "Resource sha256 must be 64 lowercase hexadecimal digits",
  ])(
    "R7: a redemption the client would not send (TypeError: %s) — unreachable, pinned as a defence — is a failure, logged as the host's fault",
    async (message) => {
      const error = vi.spyOn(console, "error").mockImplementation(() => {})
      const thrown = new TypeError(message)
      const apps = fakeApps({ fetchResource: vi.fn(() => Promise.reject(thrown)) })
      expect(await gatewayAppServer(apps).readResource(address, uri, live())).toEqual({
        kind: "failed",
      })
      expect(apps.fetchResource).toHaveBeenCalledTimes(1)
      expect(error.mock.calls).toEqual([
        ["An MCP App's resource was not fetched", thrown],
      ])
    },
  )

  // Unreachable through the real client, which answers `aborted` only for the
  // signal it was given (`mcp-apps-api.test.ts`, "reports a transport's own
  // AbortError … as unreachable"). Pinned as a defence.
  it("R8: a redemption the client answers aborted while the mount is still live — unreachable, pinned as a defence — is a failure, and, the client's code trusted as given, not logged", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const mount = new AbortController()
    const apps = fakeApps({
      fetchResource: vi.fn(() => Promise.reject(new NessaMcpResourceError("aborted"))),
    })
    expect(await gatewayAppServer(apps).readResource(address, uri, mount.signal)).toEqual(
      { kind: "failed" },
    )
    expect(mount.signal.aborted).toBe(false)
    expect(apps.fetchResource).toHaveBeenCalledTimes(1)
    expect(error).not.toHaveBeenCalled()
  })
})

describe("a read whose mount is released (R6)", () => {
  it("R6: released while the read was out, the ticket is not redeemed, and nothing is logged", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    let answer!: (value: McpReadResourceResult) => void
    const apps = fakeApps({
      readResource: vi.fn(
        () => new Promise<McpReadResourceResult>((ok) => (answer = ok)),
      ),
    })
    const mount = new AbortController()
    const read = gatewayAppServer(apps).readResource(address, uri, mount.signal)
    mount.abort()
    answer(described)
    expect(await read).toEqual({ kind: "failed" })
    expect(apps.fetchResource).not.toHaveBeenCalled()
    expect(error).not.toHaveBeenCalled()
  })

  it("R6: released while the bytes are fetched, the fetch is abandoned through the same signal, and it is no fault", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const mount = new AbortController()
    const apps = fakeApps({
      fetchResource: vi.fn(
        (_ticket, _expected, options?: { signal?: AbortSignal }) =>
          new Promise<Uint8Array<ArrayBuffer>>((_, reject) =>
            options?.signal?.addEventListener("abort", () =>
              reject(new NessaMcpResourceError("aborted")),
            ),
          ),
      ),
    })
    const read = gatewayAppServer(apps).readResource(address, uri, mount.signal)
    await new Promise((resolve) => setTimeout(resolve, 0))
    mount.abort()
    expect(await read).toEqual({ kind: "failed" })
    expect(error).not.toHaveBeenCalled()
  })

  it("R4, R6: bytes that fail their check as the mount is released are still a fault, and logged", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const mount = new AbortController()
    const apps = fakeApps({
      fetchResource: vi.fn(() => {
        mount.abort()
        return Promise.reject(new NessaMcpResourceError("integrity", 200))
      }),
    })
    expect(await gatewayAppServer(apps).readResource(address, uri, mount.signal)).toEqual(
      {
        kind: "failed",
      },
    )
    expect(error).toHaveBeenCalledTimes(1)
  })
})

describe("whatever the client throws", () => {
  it("A11, R4: callTool and readResource never reject, whatever is thrown (robustness inputs outside what the client's call() is written to throw)", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {})
    for (const thrown of [undefined, null, 0, "x", {}, new Error("x"), Symbol("x")]) {
      const reject = () => Promise.reject(thrown)
      const apps = fakeApps({
        callTool: vi.fn(reject),
        readResource: vi.fn(reject),
      })
      const server = gatewayAppServer(apps)
      await expect(server.callTool(address, "t", {})).resolves.toEqual({ kind: "failed" })
      await expect(server.readResource(address, uri, live())).resolves.toEqual({
        kind: "failed",
      })
      const fetchFails = fakeApps({ fetchResource: vi.fn(reject) })
      await expect(
        gatewayAppServer(fetchFails).readResource(address, uri, live()),
      ).resolves.toEqual({ kind: "failed" })
    }
  })
})

describe("the release", () => {
  it("M2: releases exactly this mount of this conversation's app", async () => {
    const apps = fakeApps()
    await gatewayAppServer(apps).release(address)
    expect(apps.releaseApp).toHaveBeenCalledWith(conversationId, app, {
      requestId: "release-1",
    })
  })

  it.each([
    ConversationErrorCode.ConversationNotFound,
    ConversationErrorCode.ConversationDeleted,
  ])(
    "M5b: a release refused %s — the conversation is gone — has nothing left to let go, and is no fault",
    async (code) => {
      const apps = fakeApps({
        releaseApp: vi.fn(() =>
          Promise.reject(
            new NessaConversationControlError(
              conversationId,
              "r",
              app.executionId,
              new NessaRpcError(code, "gone"),
            ),
          ),
        ),
      })
      await expect(gatewayAppServer(apps).release(address)).resolves.toBeUndefined()
    },
  )

  it("M5: a release refused conversation_closed rejects: a closed conversation reopens, and is not gone", async () => {
    const apps = fakeApps({
      releaseApp: vi.fn(() =>
        Promise.reject(
          new NessaConversationControlError(
            conversationId,
            "r",
            app.executionId,
            new NessaRpcError(ConversationErrorCode.ConversationClosed, "closed"),
          ),
        ),
      ),
    })
    await expect(gatewayAppServer(apps).release(address)).rejects.toBeInstanceOf(
      NessaConversationControlError,
    )
  })

  it("M5: a release that never lands is tried again after each wait, then rejects for the bridge to log", async () => {
    waited = []
    const apps = fakeApps({
      releaseApp: vi.fn(() => Promise.reject(new Error("closed"))),
    })
    await expect(gatewayAppServer(apps).release(address)).rejects.toThrow("closed")
    expect(apps.releaseApp).toHaveBeenCalledTimes(releaseRetryMs.length + 1)
    expect(waited).toEqual([...releaseRetryMs])
  })

  it("M5: a release whose answer was lost is asked again, and lands", async () => {
    waited = []
    const lost = new NessaConversationControlError(
      conversationId,
      "r",
      app.executionId,
      new Error("socket closed"),
    )
    expect(lost.uncertain).toBe(true)
    const releaseApp = vi
      .fn<McpAppsApi["releaseApp"]>()
      .mockRejectedValueOnce(lost)
      .mockResolvedValue({ requestId: "r", applied: true })
    const apps = fakeApps({ releaseApp })
    await expect(gatewayAppServer(apps).release(address)).resolves.toBeUndefined()
    expect(releaseApp).toHaveBeenCalledTimes(2)
    expect(waited).toEqual([releaseRetryMs[0]])
  })

  it("M5: a release refused temporarily_unavailable is asked again, and lands", async () => {
    waited = []
    const busy = new NessaConversationControlError(
      conversationId,
      "r",
      app.executionId,
      new NessaRpcError(ConversationErrorCode.TemporarilyUnavailable, "busy"),
    )
    const releaseApp = vi
      .fn<McpAppsApi["releaseApp"]>()
      .mockRejectedValueOnce(busy)
      .mockRejectedValueOnce(busy)
      .mockResolvedValue({ requestId: "r", applied: true })
    await expect(
      gatewayAppServer(fakeApps({ releaseApp })).release(address),
    ).resolves.toBeUndefined()
    expect(releaseApp).toHaveBeenCalledTimes(3)
    expect(waited).toEqual(releaseRetryMs.slice(0, 2))
  })

  it("M5b: a conversation gone on a later try ends the release, as on the first", async () => {
    waited = []
    const lost = new NessaConversationControlError(
      conversationId,
      "r",
      app.executionId,
      new Error("lost"),
    )
    const gone = new NessaConversationControlError(
      conversationId,
      "r",
      app.executionId,
      new NessaRpcError(ConversationErrorCode.ConversationDeleted, "gone"),
    )
    const releaseApp = vi
      .fn<McpAppsApi["releaseApp"]>()
      .mockRejectedValueOnce(lost)
      .mockRejectedValueOnce(gone)
    await expect(
      gatewayAppServer(fakeApps({ releaseApp })).release(address),
    ).resolves.toBeUndefined()
    expect(releaseApp).toHaveBeenCalledTimes(2)
  })

  it("M5c: every try of one release carries the same request id", async () => {
    const lost = new NessaConversationControlError(
      conversationId,
      "r",
      app.executionId,
      new Error("lost"),
    )
    const releaseApp = vi
      .fn<McpAppsApi["releaseApp"]>()
      .mockRejectedValueOnce(lost)
      .mockResolvedValue({ requestId: "r", applied: true })
    await gatewayAppServer(fakeApps({ releaseApp })).release(address)
    const ids = releaseApp.mock.calls.map((call) => call[2]?.requestId)
    expect(ids).toEqual(["release-1", "release-1"])
  })

  it("M5a: a release the client could not send is not asked again", async () => {
    waited = []
    const releaseApp = vi.fn(() => Promise.reject(new TypeError("not an app reference")))
    await expect(
      gatewayAppServer(fakeApps({ releaseApp })).release(address),
    ).rejects.toBeInstanceOf(TypeError)
    expect(releaseApp).toHaveBeenCalledTimes(1)
    expect(waited).toEqual([])
  })

  it("M5c: a lost answer the gateway applied, then the next try also applied, withdraws each review once", async () => {
    // An idempotent gateway, as `release_app` is: a release marks the mount
    // released and withdraws only the reviews still open.
    const open = new Set(["review-1", "review-2"])
    const withdrawn: string[] = []
    const apply = () => {
      for (const review of open) withdrawn.push(review)
      open.clear()
    }
    const lost = new NessaConversationControlError(
      conversationId,
      "r",
      app.executionId,
      new Error("lost"),
    )
    const releaseApp = vi
      .fn<McpAppsApi["releaseApp"]>()
      .mockImplementationOnce(async () => {
        apply()
        throw lost
      })
      .mockImplementation(async () => {
        apply()
        return { requestId: "r", applied: true }
      })
    await expect(
      gatewayAppServer(fakeApps({ releaseApp })).release(address),
    ).resolves.toBeUndefined()
    expect(releaseApp).toHaveBeenCalledTimes(2)
    expect(withdrawn).toEqual(["review-1", "review-2"])
    const ids = releaseApp.mock.calls.map((call) => call[2]?.requestId)
    expect(ids).toEqual(["release-1", "release-1"])
  })

  it("M5d: a remount while the old mount's release is retried leaves the new mount alone", async () => {
    const remounted = { ...app, instanceId: crypto.randomUUID() }
    const newMount: AppAddress = { ...address, app: remounted }
    const lost = new NessaConversationControlError(
      conversationId,
      "r",
      app.executionId,
      new Error("lost"),
    )
    const releaseApp = vi
      .fn<McpAppsApi["releaseApp"]>()
      .mockRejectedValueOnce(lost)
      .mockRejectedValueOnce(lost)
      .mockResolvedValue({ requestId: "r", applied: true })
    const apps = fakeApps({ releaseApp })
    // A clock whose waits run only when the test says, so the new mount
    // calls and reads while the old mount's release is between tries.
    const waits: (() => void)[] = []
    const server = serverOver(
      apps,
      (_ms, run) => {
        waits.push(run)
        return () => {}
      },
      () => "release-1",
    )
    const released = server.release(address)
    for (const tried of [1, 2]) {
      await new Promise((resolve) => setTimeout(resolve, 0))
      expect(waits).toHaveLength(tried)
      expect(await server.callTool(newMount, "get_weather", {})).toEqual({
        kind: "ok",
        result: { content: [{ type: "text", text: "72" }] },
      })
      expect((await server.readResource(newMount, uri, live())).kind).toBe("ok")
      waits[tried - 1]!()
    }
    await expect(released).resolves.toBeUndefined()
    expect(releaseApp.mock.calls.map((call) => call[1])).toEqual([app, app, app])
    expect(apps.callTool.mock.calls.map((call) => call[1])).toEqual([
      remounted,
      remounted,
    ])
    expect(apps.readResource.mock.calls.map((call) => call[1])).toEqual([
      remounted,
      remounted,
    ])
  })

  it("M8: a release acknowledged applied false (an input the gateway does not send) is done, not asked again", async () => {
    waited = []
    const releaseApp = vi.fn<McpAppsApi["releaseApp"]>(async () => ({
      requestId: "release-1",
      applied: false,
    }))
    await expect(
      gatewayAppServer(fakeApps({ releaseApp })).release(address),
    ).resolves.toBeUndefined()
    expect(releaseApp).toHaveBeenCalledTimes(1)
    expect(waited).toEqual([])
  })

  it("M5a: a release the gateway refused outright is not asked again", async () => {
    waited = []
    const releaseApp = vi.fn(() =>
      Promise.reject(
        new NessaConversationControlError(
          conversationId,
          "r",
          app.executionId,
          new NessaRpcError(ConversationErrorCode.InvalidRequest, "no"),
        ),
      ),
    )
    await expect(
      gatewayAppServer(fakeApps({ releaseApp })).release(address),
    ).rejects.toBeInstanceOf(NessaConversationControlError)
    expect(releaseApp).toHaveBeenCalledTimes(1)
    expect(waited).toEqual([])
  })
})

describe("#349's L14 and L24, through the real bridge over this adapter", () => {
  const mount = "1c2d3e4f-5a6b-4c7d-8e9f-0a1b2c3d4e5f"

  function bridged(apps: ReturnType<typeof fakeApps>) {
    const posted: Outgoing[] = []
    const views: AppViewState[] = []
    const ports: McpAppPorts = {
      server: gatewayAppServer(apps),
      calls: { read: () => ({ kind: "missing" }), subscribe: () => () => {} },
      timers: { after: () => () => {} },
      newId: () => mount,
      sandbox: {
        url: "http://127.0.0.1:9999/proxy.html",
        origin: "http://127.0.0.1:9999",
      },
      hostInfo: { name: "Nessa", version: "test" },
      page: () => ({ styles: {}, timeZone: "UTC", platform: "desktop" }),
    }
    const bridge = createAppBridge({
      place: "pane",
      server: "weather",
      call: {
        ...fixtureCall(conversationId),
        executionId: app.executionId,
        toolId: app.toolId,
        resourceUri: uri,
      },
      context: {
        theme: "light",
        locale: "en-US",
        size: { width: 400, height: 300 },
        safeArea: { top: 0, right: 0, bottom: 0, left: 0 },
      },
      ports,
      post: (message) => void posted.push(message),
      host: { open: () => {}, close: () => {} },
      onView: (view) => void views.push(view),
    })
    const say = (data: unknown) => bridge.receive(readFromFrame(readEnvelope(data)))
    return { bridge, posted, views, say }
  }

  const flush = () => new Promise((resolve) => setTimeout(resolve, 0))
  const ownApp = { ...app, instanceId: mount }

  async function live(view: ReturnType<typeof bridged>) {
    await flush()
    view.say({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-proxy-ready",
      params: {},
    })
    view.say({
      jsonrpc: "2.0",
      id: 1,
      method: "ui/initialize",
      params: {
        appInfo: { name: "Weather", version: "1" },
        appCapabilities: {},
        protocolVersion: "2026-01-26",
      },
    })
    view.say({ jsonrpc: "2.0", method: "ui/notifications/initialized" })
    await flush()
    view.posted.length = 0
  }

  it("L14: the app's tools/call reaches the gateway as the view's own app and mount; a refusal says the gateway's reason", async () => {
    const apps = fakeApps()
    const view = bridged(apps)
    await live(view)
    expect(apps.readResource).toHaveBeenCalledWith(conversationId, ownApp, "weather", uri)
    view.say({
      jsonrpc: "2.0",
      id: 2,
      method: "tools/call",
      params: {
        name: "get_weather",
        arguments: { city: "Oslo" },
        _meta: { app: { ...app, instanceId: "forged" }, server: "other" },
      },
    })
    apps.callTool.mockRejectedValueOnce(refusal(ConversationErrorCode.McpToolNotForApp))
    view.say({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "secret" } })
    apps.callTool.mockRejectedValueOnce(refusal(ConversationErrorCode.McpApprovalDenied))
    view.say({
      jsonrpc: "2.0",
      id: 4,
      method: "tools/call",
      params: { name: "delete_all" },
    })
    await flush()
    expect(apps.callTool.mock.calls).toEqual([
      [conversationId, ownApp, "weather", "get_weather", '{"city":"Oslo"}'],
      [conversationId, ownApp, "weather", "secret", "{}"],
      [conversationId, ownApp, "weather", "delete_all", "{}"],
    ])
    expect(view.posted).toEqual([
      { jsonrpc: "2.0", id: 2, result: { content: [{ type: "text", text: "72" }] } },
      {
        jsonrpc: "2.0",
        id: 3,
        error: { code: -32000, message: "This app may not use that tool" },
      },
      {
        jsonrpc: "2.0",
        id: 4,
        error: { code: -32000, message: "The person declined this action" },
      },
    ])
  })

  it("L24, A12: the place removed while calls wait releases the mount; their answers after are neither posted nor shown", async () => {
    const apps = fakeApps()
    const answers: ((error: unknown) => void)[] = []
    const held = () => new Promise<never>((_, reject) => void answers.push(reject))
    apps.callTool.mockImplementationOnce(held).mockImplementationOnce(held)
    const view = bridged(apps)
    await live(view)
    for (const id of [2, 3])
      view.say({
        jsonrpc: "2.0",
        id,
        method: "tools/call",
        params: { name: "delete_all" },
      })
    await flush()
    view.bridge.remove()
    await flush()
    expect(apps.releaseApp).toHaveBeenCalledTimes(1)
    expect(apps.releaseApp).toHaveBeenCalledWith(conversationId, ownApp, {
      requestId: "release-1",
    })
    expect(view.views.at(-1)?.lifecycle).toEqual({ kind: "gone" })
    const shown = view.views.length
    // The gateway answers the withdrawn review's call, and the other's server
    // is gone — which, answered live, would show the server-gone notice.
    answers[0]!(refusal(ConversationErrorCode.McpCancelled))
    answers[1]!(refusal(ConversationErrorCode.McpSessionUnavailable))
    await flush()
    expect(view.posted).toEqual([])
    expect(view.views).toHaveLength(shown)
    expect(view.bridge.view()).toMatchObject({ lifecycle: { kind: "gone" } })
    expect(view.bridge.view().serverGone).toBeFalsy()
  })

  it("L24, R6: the place removed while the app is read releases the mount, and its ticket is never redeemed", async () => {
    let answer!: (value: McpReadResourceResult) => void
    const apps = fakeApps({
      readResource: vi.fn(
        () => new Promise<McpReadResourceResult>((ok) => (answer = ok)),
      ),
    })
    const view = bridged(apps)
    await flush()
    view.bridge.remove()
    answer(described)
    await flush()
    expect(apps.releaseApp).toHaveBeenCalledWith(conversationId, ownApp, {
      requestId: "release-1",
    })
    expect(apps.fetchResource).not.toHaveBeenCalled()
  })

  it("M2: an app that leaves its frame while a review waits is released at once, not when its card goes", async () => {
    const apps = fakeApps()
    apps.callTool.mockImplementationOnce(() => new Promise(() => {}))
    const view = bridged(apps)
    await live(view)
    view.say({
      jsonrpc: "2.0",
      id: 2,
      method: "tools/call",
      params: { name: "delete_all" },
    })
    view.say({ jsonrpc: "2.0", method: "ui/notifications/sandbox-app-left", params: {} })
    await flush()
    expect(view.bridge.view().lifecycle).toEqual({ kind: "failed", reason: "load" })
    expect(apps.releaseApp).toHaveBeenCalledTimes(1)
    expect(apps.releaseApp).toHaveBeenCalledWith(conversationId, ownApp, {
      requestId: "release-1",
    })
    view.bridge.remove()
    await flush()
    expect(apps.releaseApp).toHaveBeenCalledTimes(1)
  })

  /** The app's `tools/call` with `id` 2, answered as `apps.callTool` next answers; what is posted. */
  async function called(apps: ReturnType<typeof fakeApps>) {
    const view = bridged(apps)
    await live(view)
    view.say({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "t" } })
    await flush()
    return view
  }

  it("J1: a call that fails with no code the gateway placed reaches the app as -32603, and is logged as a fault", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const lost = new NessaMcpAppError(
      conversationId,
      "r",
      app,
      new Error("socket closed"),
    )
    const apps = fakeApps()
    apps.callTool.mockRejectedValueOnce(lost)
    const view = await called(apps)
    expect(view.posted).toEqual([
      { jsonrpc: "2.0", id: 2, error: { code: -32603, message: "The request failed" } },
    ])
    expect(error.mock.calls).toEqual([["An MCP App call failed", lost]])
    expect(view.bridge.view()).toMatchObject({ lifecycle: { kind: "live" } })
  })

  it("J2: the server's own JSON-RPC error reaches the app with its signed code and message as the server sent them", async () => {
    const apps = fakeApps()
    apps.callTool.mockRejectedValueOnce(
      refusal(ConversationErrorCode.McpRemoteError, {
        code: -32002,
        message: "Resource not found",
      }),
    )
    const view = await called(apps)
    expect(view.posted).toEqual([
      { jsonrpc: "2.0", id: 2, error: { code: -32002, message: "Resource not found" } },
    ])
  })

  it("J3: a server session gone reaches the app as an error, and the view shows the server-gone notice", async () => {
    const apps = fakeApps()
    apps.callTool.mockRejectedValueOnce(
      refusal(ConversationErrorCode.McpSessionUnavailable),
    )
    const view = bridged(apps)
    await live(view)
    expect(view.bridge.view().serverGone).toBeFalsy()
    view.say({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "t" } })
    await flush()
    expect(view.posted).toEqual([
      {
        jsonrpc: "2.0",
        id: 2,
        error: { code: -32000, message: "The app's server has stopped" },
      },
    ])
    expect(view.bridge.view()).toMatchObject({
      lifecycle: { kind: "live" },
      serverGone: true,
    })
  })

  it("J4: no room on the gateway's app lane reaches the app as refused, too many requests at once", async () => {
    const apps = fakeApps()
    apps.callTool.mockRejectedValueOnce(
      refusal(ConversationErrorCode.TemporarilyUnavailable),
    )
    const view = await called(apps)
    expect(view.posted).toEqual([
      {
        jsonrpc: "2.0",
        id: 2,
        error: { code: -32000, message: "Too many requests at once" },
      },
    ])
    expect(view.bridge.view().serverGone).toBeFalsy()
  })

  it("J5: the app's own resources/read after live gets the resource, never the ticket; refused, failed, busy and server-gone reads get the call's errors", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const apps = fakeApps()
    const view = bridged(apps)
    await live(view)
    const read = async (id: number) => {
      view.say({ jsonrpc: "2.0", id, method: "resources/read", params: { uri } })
      await flush()
    }
    await read(2)
    apps.readResource.mockRejectedValueOnce(refusal(ConversationErrorCode.McpAppUnknown))
    await read(3)
    const lost = new NessaMcpAppError(
      conversationId,
      "r",
      app,
      new Error("socket closed"),
    )
    apps.readResource.mockRejectedValueOnce(lost)
    await read(4)
    apps.readResource.mockRejectedValueOnce(
      refusal(ConversationErrorCode.TemporarilyUnavailable),
    )
    await read(5)
    expect(view.bridge.view().serverGone).toBeFalsy()
    apps.readResource.mockRejectedValueOnce(
      refusal(ConversationErrorCode.McpSessionUnavailable),
    )
    await read(6)
    // The first read (the mount's) and the app's own five, all as its own mount.
    expect(apps.readResource.mock.calls).toEqual(
      Array.from({ length: 6 }, () => [conversationId, ownApp, "weather", uri]),
    )
    expect(apps.fetchResource).toHaveBeenCalledTimes(2)
    expect(view.posted).toEqual([
      {
        jsonrpc: "2.0",
        id: 2,
        result: {
          contents: [
            {
              uri,
              mimeType: described.mimeType,
              text: page,
              _meta: {
                ui: {
                  csp: {
                    connectDomains: ["https://api.weather.example"],
                    resourceDomains: [],
                    frameDomains: [],
                    baseUriDomains: [],
                  },
                  permissions: described.permissions,
                },
              },
            },
          ],
        },
      },
      {
        jsonrpc: "2.0",
        id: 3,
        error: { code: -32000, message: "This app may not read that resource" },
      },
      { jsonrpc: "2.0", id: 4, error: { code: -32603, message: "The request failed" } },
      {
        jsonrpc: "2.0",
        id: 5,
        error: { code: -32000, message: "Too many requests at once" },
      },
      {
        jsonrpc: "2.0",
        id: 6,
        error: { code: -32000, message: "The app's server has stopped" },
      },
    ])
    expect(JSON.stringify(view.posted)).not.toContain(ticket)
    expect(view.bridge.view()).toMatchObject({
      lifecycle: { kind: "live" },
      serverGone: true,
    })
    // Only the unplaced fault is logged.
    expect(error.mock.calls).toEqual([["An MCP App call failed", lost]])
  })
})
