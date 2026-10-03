import { createHash } from "node:crypto"
import { describe, expect, it, vi } from "vitest"

import { NessaConversationControlError } from "../application/conversation-mutation-error.js"
import type { RequestTimer } from "../application/gateway-http.js"
import {
  MCP_APP_CALL_DEADLINE_MS,
  NessaMcpAppError,
} from "../application/mcp-app-call.js"
import {
  NessaMcpResourceError,
  RESOURCE_DEADLINE_MS,
  type McpResourceReply,
  type McpResourceTransport,
} from "../application/mcp-resource-fetch.js"
import { NessaRequestTooLargeError } from "../application/request-too-large-error.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { conversationView } from "../protocol/conversation-validate.js"
import { mcpAppRequestProblem } from "../protocol/mcp-app-validate.js"
import { createMcpAppsApi } from "./mcp-apps-api.js"

const conversationId = "00000000-0000-4000-8000-000000000001"
const app = {
  executionId: "execution",
  toolId: "tool-call",
  instanceId: "00000000-0000-4000-8000-0000000000aa",
}
const ticket = "Tk_-".repeat(10) + "abc"
const uri = "ui://charts/app.html"
const html = new TextEncoder().encode("<!doctype html><p>chart</p>")
const sha256 = createHash("sha256").update(html).digest("hex")
const resource = {
  uri,
  mimeType: "text/html;profile=mcp-app",
  size: html.byteLength,
  sha256,
  ticket,
  expiresInMs: 60_000,
  csp: {
    connectDomains: ["https://api.example"],
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
}

/** A deadline clock the test fires by hand. Nothing here waits. */
function manualTimer() {
  const pending: { ms: number; elapsed: () => void; cancelled: boolean }[] = []
  const timer: RequestTimer = (ms, elapsed) => {
    const entry = { ms, elapsed, cancelled: false }
    pending.push(entry)
    return () => {
      entry.cancelled = true
    }
  }
  return { timer, pending }
}
const unused: McpResourceTransport = {
  get: () => Promise.reject(new Error("no fetch expected")),
}
/** What the session port is asked, as the mocks record it. */
type Request = (method: string, params: unknown, deadline?: unknown) => Promise<unknown>
function api(request: Request) {
  return createMcpAppsApi({ request }, unused, () => "generated", manualTimer().timer)
}
function fetches(get: McpResourceTransport["get"], clock = manualTimer()) {
  return createMcpAppsApi({ request: vi.fn() }, { get }, () => "id", clock.timer)
}
const failure = (promise: Promise<unknown>) => promise.catch((error: unknown) => error)

it("calls the app's tool with exactly its arguments, and waits as long as a review can", async () => {
  const request = vi.fn(async () => ({ resultJson: '{"content":[]}' }))
  const result = await api(request).callTool(
    conversationId,
    app,
    "charts",
    "delete_rows",
    '{"rows":[1]}',
    { requestId: "call" },
  )
  expect(result).toEqual({ resultJson: '{"content":[]}' })
  expect(request).toHaveBeenCalledExactlyOnceWith(
    "mcp.callTool",
    {
      conversationId,
      requestId: "call",
      app,
      server: "charts",
      tool: "delete_rows",
      argumentsJson: '{"rows":[1]}',
    },
    { atLeastMs: MCP_APP_CALL_DEADLINE_MS },
  )
  // Five minutes of review, a minute of call, and a margin.
  expect(MCP_APP_CALL_DEADLINE_MS).toBe(370_000)
})

it("leaves out arguments that were not given, rather than sending them as nothing", async () => {
  const request = vi.fn<Request>(async () => ({ resultJson: '{"isError":true}' }))
  // isError is a result for the app, not a refusal.
  expect(await api(request).callTool(conversationId, app, "charts", "list")).toEqual({
    resultJson: '{"isError":true}',
  })
  expect(request.mock.calls[0]![1]).toEqual({
    conversationId,
    requestId: "generated",
    app,
    server: "charts",
    tool: "list",
  })
})

it("sends a copy of the app, so the caller cannot change it under the call", async () => {
  const request = vi.fn<Request>(async () => ({ resultJson: "{}" }))
  const mine = { ...app }
  await api(request).callTool(conversationId, mine, "charts", "list")
  mine.toolId = "changed"
  expect((request.mock.calls[0]![1] as { app: unknown }).app).toEqual(app)
})

const over = (bytes: number) => "x".repeat(bytes)
it.each([
  ["a non-canonical conversation", { conversationId: "Conversation" }],
  ["a blank request ID", { requestId: " " }],
  ["a request ID past 256 bytes", { requestId: over(257) }],
  ["an empty execution ID", { app: { ...app, executionId: "" } }],
  ["an execution ID past 256 bytes", { app: { ...app, executionId: over(257) } }],
  ["an empty tool call ID", { app: { ...app, toolId: "" } }],
  ["a tool call ID past 256 bytes", { app: { ...app, toolId: "é".repeat(129) } }],
  [
    "an uppercase instance ID",
    { app: { ...app, instanceId: app.instanceId.toUpperCase() } },
  ],
  ["an instance ID that is no UUID", { app: { ...app, instanceId: "mount-1" } }],
  ["an app with an unknown field", { app: { ...app, server: "charts" } }],
  ["an app that is no object", { app: null }],
  ["an empty server", { server: "" }],
  ["a server past 128 bytes", { server: over(129) }],
  ["an empty tool", { tool: "" }],
  ["a tool past 128 bytes", { tool: "é".repeat(65) }],
  ["arguments past 32 KiB", { argumentsJson: `{"a":"${over(32761)}"}` }],
  // Counted in UTF-8 bytes: 16,392 characters and 32,776 bytes.
  ["arguments past 32 KiB of UTF-8", { argumentsJson: `{"a":"${"é".repeat(16384)}"}` }],
  // A lone surrogate is no Unicode: the gateway cannot decode the frame.
  ["a tool with a lone surrogate", { tool: "get\ud800" }],
  // Written into the arguments' text itself, it would break the frame too.
  ["arguments holding a lone surrogate", { argumentsJson: '{"a":"\ud800"}' }],
  ["a tool that is no string", { tool: 7 as unknown as string }],
  ["arguments that are no string", { argumentsJson: {} as unknown as string }],
] as const)(
  "refuses to call a tool with %s before asking the gateway",
  async (_name, change) => {
    const request = vi.fn()
    const args = {
      conversationId,
      app: app as unknown,
      server: "charts",
      tool: "list",
      argumentsJson: undefined as string | undefined,
      requestId: "call",
      ...change,
    }
    await expect(
      api(request).callTool(
        args.conversationId,
        args.app as typeof app,
        args.server,
        args.tool,
        args.argumentsJson,
        { requestId: args.requestId },
      ),
    ).rejects.toBeInstanceOf(TypeError)
    expect(request).not.toHaveBeenCalled()
  },
)

it("calls a tool at exactly every bound", async () => {
  const request = vi.fn(async () => ({ resultJson: "{}" }))
  await api(request).callTool(
    conversationId,
    { ...app, executionId: over(256), toolId: "é".repeat(128) },
    over(128),
    "é".repeat(64),
    `{"a":"${over(32760)}"}`,
    { requestId: over(256) },
  )
  expect(request).toHaveBeenCalledTimes(1)
})

/** The smallest whole view the client accepts, holding one review an app asked for. */
function viewWithAppReview(argumentsJson: string) {
  return {
    conversationId,
    title: null,
    revision: "1",
    approvalMode: "ask",
    approvalModes: [{ id: "ask", name: "Ask", description: "Asks." }],
    truncated: false,
    queueComplete: true,
    transcriptState: "complete",
    messages: [
      {
        executionId: app.executionId,
        userText: "chart it",
        attachments: [],
        files: [],
        status: "completed",
        parts: [],
      },
    ],
    pending: [],
    permissions: [
      {
        executionId: app.executionId,
        permissionId: "review",
        toolId: app.toolId,
        title: "delete_rows",
        toolName: "delete_rows",
        argumentsJson,
        origin: { kind: "app", server: "charts", tool: "delete_rows" },
        options: [{ id: "allow", label: "Allow", effect: "allow" }],
      },
    ],
    questions: [],
    tools: [],
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures: {
        permissionDenial: "unknown",
        nativeHookSuppression: "unknown",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unsupported_not_implemented",
        policyEndTurn: "unsupported_not_implemented",
        policyCloseSession: "unsupported_not_implemented",
        incomingElicitation: "unsupported",
      },
    },
    lifecycle: { phase: "attached" },
  }
}

it("sends no arguments its review could not show: 32 KiB fits the view, one byte more is never sent", async () => {
  const request = vi.fn<Request>(async () => ({ resultJson: "{}" }))
  // {"a":"…"} is 8 bytes around the padding.
  const largest = `{"a":"${over(32760)}"}`
  expect(new TextEncoder().encode(largest).byteLength).toBe(32768)
  await api(request).callTool(conversationId, app, "charts", "delete_rows", largest)
  const sent = (request.mock.calls[0]![1] as { argumentsJson: string }).argumentsJson
  // The review the gateway would show for it is one the client can read.
  const view = conversationView(viewWithAppReview(sent), conversationId)
  expect(view.permissions[0]).toMatchObject({
    argumentsJson: largest,
    origin: { kind: "app", server: "charts", tool: "delete_rows" },
  })

  const refused = vi.fn()
  await expect(
    api(refused).callTool(
      conversationId,
      app,
      "charts",
      "delete_rows",
      `{"a":"${over(32761)}"}`,
    ),
  ).rejects.toBeInstanceOf(TypeError)
  expect(refused).not.toHaveBeenCalled()
})

it.each([
  ["nothing", undefined],
  ["not an object", "{}"],
  ["missing its result", {}],
  ["an unknown field", { resultJson: "{}", isError: false }],
  ["a result that is no string", { resultJson: {} }],
  ["a result that is no JSON", { resultJson: "{" }],
  ["a result that is a JSON list", { resultJson: "[]" }],
  ["a result that is a JSON string", { resultJson: '"{}"' }],
  ["a result past 56 KiB", { resultJson: `{"a":"${over(57337)}"}` }],
  ["a result past 56 KiB of UTF-8", { resultJson: `{"a":"${"é".repeat(28669)}"}` }],
])("refuses a tool answer that is %s", async (_name, reply) => {
  const error = await failure(
    api(async () => reply).callTool(conversationId, app, "charts", "list", undefined, {
      requestId: "call",
    }),
  )
  expect(error).toBeInstanceOf(NessaMcpAppError)
  // A reply nobody believes says nothing about whether the server was asked.
  expect(error).toMatchObject({
    code: undefined,
    uncertain: true,
    conversationId,
    requestId: "call",
    app,
  })
})

it("takes a tool answer of exactly 56 KiB", async () => {
  const resultJson = `{"a":"${over(57336)}"}`
  expect(new TextEncoder().encode(resultJson).byteLength).toBe(57344)
  expect(
    await api(async () => ({ resultJson })).callTool(
      conversationId,
      app,
      "charts",
      "list",
    ),
  ).toEqual({ resultJson })
})

it.each([
  "mcp_app_unknown",
  "mcp_server_mismatch",
  "mcp_tool_not_for_app",
  "mcp_request_too_large",
  "mcp_approval_denied",
  "mcp_approval_expired",
  "mcp_cancelled",
  "invalid_request",
  "conversation_not_found",
] as const)("reports %s as refused before anything reached the server", async (code) => {
  const cause = new NessaRpcError(code, "mcp_timed_out")
  const error = await failure(
    api(() => Promise.reject(cause)).callTool(conversationId, app, "charts", "list"),
  )
  expect(error).toBeInstanceOf(NessaMcpAppError)
  expect(error).toMatchObject({ code, uncertain: false, cause, remoteError: undefined })
})

it.each([
  "mcp_session_unavailable",
  "mcp_timed_out",
  "mcp_remote_error",
  "mcp_result_too_large",
  "temporarily_unavailable",
] as const)("reports %s as possibly having reached the server", async (code) => {
  const error = await failure(
    api(() => Promise.reject(new NessaRpcError(code, "mcp_cancelled"))).callTool(
      conversationId,
      app,
      "charts",
      "list",
    ),
  )
  expect(error).toMatchObject({ code, uncertain: true })
})

it("keeps no code for one this client was not taught, or for no answer at all", async () => {
  for (const cause of [
    new NessaRpcError("mcp_server_on_fire", "mcp_cancelled"),
    new NessaRpcError("toString", "mcp_cancelled"),
    new Error("mcp_cancelled"),
  ]) {
    const error = await failure(
      api(() => Promise.reject(cause)).callTool(conversationId, app, "charts", "list"),
    )
    expect(error).toMatchObject({ code: undefined, uncertain: true, cause })
  }
})

it("carries the server's own JSON-RPC error with mcp_remote_error, signed code and all", async () => {
  const details = { code: -32602, message: "Invalid params: rows" }
  const error = await failure(
    api(() =>
      Promise.reject(new NessaRpcError("mcp_remote_error", "remote", details)),
    ).callTool(conversationId, app, "charts", "delete_rows", "{}"),
  )
  expect(error).toMatchObject({ code: "mcp_remote_error", remoteError: details })
  expect((error as NessaMcpAppError).remoteError).not.toBe(details)
})

it.each([
  ["absent: an answer that was no MCP answer", undefined],
  ["not an object", "Invalid params"],
  ["missing its message", { code: -32602 }],
  ["a fractional code", { code: -1.5, message: "x" }],
  ["a code past a safe integer", { code: 2 ** 53, message: "x" }],
  ["a code that is a string", { code: "-32602", message: "x" }],
  ["a message past 512 characters", { code: 1, message: over(513) }],
  ["an unknown field", { code: 1, message: "x", data: {} }],
])("keeps mcp_remote_error without details that are %s", async (_name, details) => {
  const error = await failure(
    api(() =>
      Promise.reject(new NessaRpcError("mcp_remote_error", "remote", details)),
    ).callTool(conversationId, app, "charts", "list"),
  )
  expect(error).toMatchObject({ code: "mcp_remote_error", remoteError: undefined })
})

it("reads remote details at exactly their bounds, and only beside mcp_remote_error", async () => {
  // 512 characters of four bytes each: the character bound, and the byte bound with it.
  const message = "😀".repeat(512)
  const details = { code: -(2 ** 53 - 1), message }
  const error = await failure(
    api(() =>
      Promise.reject(new NessaRpcError("mcp_remote_error", "remote", details)),
    ).callTool(conversationId, app, "charts", "list"),
  )
  expect(error).toMatchObject({ remoteError: details })
  const other = await failure(
    api(() => Promise.reject(new NessaRpcError("mcp_timed_out", "x", details))).callTool(
      conversationId,
      app,
      "charts",
      "list",
    ),
  )
  expect(other).toMatchObject({ code: "mcp_timed_out", remoteError: undefined })
})

it("reads a resource and returns what the gateway holds, ticket and all", async () => {
  const request = vi.fn(async () => ({
    ...resource,
    domain: "app.example",
    prefersBorder: false,
  }))
  const answer = await api(request).readResource(conversationId, app, "charts", uri, {
    requestId: "read",
  })
  expect(request).toHaveBeenCalledExactlyOnceWith(
    "mcp.readResource",
    {
      conversationId,
      requestId: "read",
      app,
      server: "charts",
      uri,
    },
    // The gateway may open the conversation first: no shorter wait than a
    // call's.
    { atLeastMs: MCP_APP_CALL_DEADLINE_MS },
  )
  expect(answer).toEqual({ ...resource, domain: "app.example", prefersBorder: false })
  // Absent is absent: neither is invented when the app did not say.
  const plain = await api(async () => resource).readResource(
    conversationId,
    app,
    "charts",
    uri,
  )
  expect(plain).toEqual(resource)
  expect("domain" in plain || "prefersBorder" in plain).toBe(false)
})

it("reads a resource at exactly its bounds", async () => {
  const longest = `ui://${over(2043)}`
  const origins = Array.from({ length: 64 }, () => `https://${over(504)}`)
  const answer = {
    ...resource,
    uri: longest,
    size: 4 * 1024 * 1024,
    csp: { ...resource.csp, frameDomains: origins },
    domain: over(512),
  }
  expect(
    await api(async () => answer).readResource(conversationId, app, over(128), longest),
  ).toEqual(answer)
  expect(
    await api(async () => ({ ...resource, size: 0 })).readResource(
      conversationId,
      app,
      "charts",
      uri,
    ),
  ).toMatchObject({ size: 0 })
})

it.each([
  ["an empty URI", { uri: "" }],
  ["a URI past 2048 bytes", { uri: `ui://${"é".repeat(1022)}` }],
  ["an empty server", { server: "" }],
  ["a server past 128 bytes", { server: over(129) }],
  ["an app with no instance", { app: { executionId: "e", toolId: "t" } }],
] as const)(
  "refuses to read a resource with %s before asking the gateway",
  async (_name, change) => {
    const request = vi.fn()
    const args = { app: app as unknown, server: "charts", uri, ...change }
    await expect(
      api(request).readResource(
        conversationId,
        args.app as typeof app,
        args.server,
        args.uri,
      ),
    ).rejects.toBeInstanceOf(TypeError)
    expect(request).not.toHaveBeenCalled()
  },
)

it.each([
  ["another resource", { ...resource, uri: "ui://charts/other.html" }],
  ["another media type", { ...resource, mimeType: "text/html" }],
  ["past 4 MiB", { ...resource, size: 4 * 1024 * 1024 + 1 }],
  ["a negative size", { ...resource, size: -1 }],
  ["a fractional size", { ...resource, size: 1.5 }],
  ["an uppercase digest", { ...resource, sha256: sha256.toUpperCase() }],
  ["a short digest", { ...resource, sha256: sha256.slice(1) }],
  ["a short ticket", { ...resource, ticket: ticket.slice(1) }],
  ["a ticket outside base64url", { ...resource, ticket: `${ticket.slice(1)}+` }],
  ["another ticket lifetime", { ...resource, expiresInMs: 30_000 }],
  ["no ticket", { ...resource, ticket: undefined }],
  ["no CSP", { ...resource, csp: undefined }],
  [
    "a CSP missing a list",
    { ...resource, csp: { connectDomains: [], resourceDomains: [], frameDomains: [] } },
  ],
  [
    "a CSP with an unknown list",
    { ...resource, csp: { ...resource.csp, scriptDomains: [] } },
  ],
  [
    "a CSP list past 64",
    {
      ...resource,
      csp: { ...resource.csp, connectDomains: Array(65).fill("https://a") },
    },
  ],
  ["an empty CSP origin", { ...resource, csp: { ...resource.csp, frameDomains: [""] } }],
  [
    "a CSP origin past 512 bytes",
    { ...resource, csp: { ...resource.csp, frameDomains: [over(513)] } },
  ],
  [
    "a CSP origin that is no string",
    { ...resource, csp: { ...resource.csp, frameDomains: [1] } },
  ],
  ["no permissions", { ...resource, permissions: undefined }],
  [
    "a permission missing",
    {
      ...resource,
      permissions: { camera: false, microphone: false, geolocation: false },
    },
  ],
  [
    "a permission that is no flag",
    { ...resource, permissions: { ...resource.permissions, camera: {} } },
  ],
  [
    "an unknown permission",
    { ...resource, permissions: { ...resource.permissions, usb: false } },
  ],
  ["a null domain", { ...resource, domain: null }],
  ["an empty domain", { ...resource, domain: "" }],
  ["a domain past 512 bytes", { ...resource, domain: over(513) }],
  ["a null prefersBorder", { ...resource, prefersBorder: null }],
  ["a prefersBorder that is no flag", { ...resource, prefersBorder: "yes" }],
  ["an unknown field", { ...resource, url: `https://gateway/mcp-resources?t=${ticket}` }],
  ["a list", [resource]],
])("refuses a resource answer naming %s", async (_name, reply) => {
  const error = await failure(
    api(async () => reply).readResource(conversationId, app, "charts", uri),
  )
  expect(error).toBeInstanceOf(NessaMcpAppError)
  expect(error).toMatchObject({ code: undefined, uncertain: true })
  // The ticket is a secret; a malformed reply is no reason to print one.
  expect(String((error as Error).message)).not.toContain(ticket)
  expect(String(((error as Error).cause as Error).message)).not.toContain(ticket)
})

it("reports a resource that is not an app's HTML as refused before anything changed", async () => {
  const error = await failure(
    api(() => Promise.reject(new NessaRpcError("mcp_app_unknown", "x"))).readResource(
      conversationId,
      app,
      "charts",
      "ui://charts/data.json",
    ),
  )
  expect(error).toMatchObject({ code: "mcp_app_unknown", uncertain: false })
})

it("fetches the described bytes by ticket and hands them back once they match", async () => {
  const clock = manualTimer()
  const get = vi.fn(async () => ({ status: 200, bytes: html.slice() }))
  const bytes = await fetches(get, clock).fetchResource(ticket, {
    size: html.byteLength,
    sha256,
  })
  expect(bytes).toEqual(html)
  expect(get).toHaveBeenCalledExactlyOnceWith({
    ticket,
    maxBytes: html.byteLength,
    signal: expect.any(AbortSignal),
  })
  // An answered fetch leaves no deadline running behind it.
  expect(clock.pending).toMatchObject([{ ms: RESOURCE_DEADLINE_MS, cancelled: true }])
  // An empty resource is still a resource.
  const empty = createHash("sha256").update(new Uint8Array()).digest("hex")
  expect(
    await fetches(async () => ({ status: 200, bytes: new Uint8Array() })).fetchResource(
      ticket,
      { size: 0, sha256: empty },
    ),
  ).toEqual(new Uint8Array())
})

it.each([
  [
    "other bytes of the same length",
    (() => {
      const other = html.slice()
      other[0] = other[0]! ^ 1
      return other
    })(),
  ],
  ["fewer bytes", html.slice(1)],
  ["more bytes", new Uint8Array([...html, 0])],
  ["no bytes", new Uint8Array()],
])("refuses %s than the ones described as integrity", async (_name, bytes) => {
  const error = await failure(
    fetches(async () => ({ status: 200, bytes })).fetchResource(ticket, {
      size: html.byteLength,
      sha256,
    }),
  )
  expect(error).toBeInstanceOf(NessaMcpResourceError)
  expect(error).toMatchObject({ code: "integrity", status: 200 })
})

it("refuses the described bytes under a description whose size disagrees with its digest", async () => {
  // The digest matches what arrived; the size does not. Both are the description.
  const error = await failure(
    fetches(async () => ({ status: 200, bytes: html.slice() })).fetchResource(ticket, {
      size: html.byteLength + 1,
      sha256,
    }),
  )
  expect(error).toMatchObject({ code: "integrity", status: 200 })
})

it.each([
  [404, "not_found"],
  [503, "unavailable"],
  [403, "unexpected_response"],
  [500, "unexpected_response"],
  // The bytes under a refusing status are not a success.
  [204, "unexpected_response"],
] as const)("maps resource answer %i to %s", async (status, code) => {
  const error = await failure(
    fetches(async () => ({ status, bytes: html.slice() })).fetchResource(ticket, {
      size: html.byteLength,
      sha256,
    }),
  )
  expect(error).toBeInstanceOf(NessaMcpResourceError)
  expect(error).toMatchObject({ code, status })
  expect(String((error as Error).message)).not.toContain(ticket)
})

it("reports a fetch that got no answer as unreachable, keeping the cause", async () => {
  const cause = new TypeError("Failed to fetch")
  const error = await failure(
    fetches(() => Promise.reject(cause)).fetchResource(ticket, { size: 1, sha256 }),
  )
  expect(error).toMatchObject({ code: "unreachable", cause, status: undefined })
})

it("gives up on a fetch that never answers when its deadline elapses, and aborts it", async () => {
  const clock = manualTimer()
  let requestSignal: AbortSignal | undefined
  // Never settles, and ignores the abort: the wait must end all the same.
  const get = vi.fn((request: { signal?: AbortSignal }) => {
    requestSignal = request.signal
    return new Promise<McpResourceReply>(() => {})
  })
  const pending = failure(
    fetches(get, clock).fetchResource(ticket, { size: html.byteLength, sha256 }),
  )
  expect(clock.pending).toMatchObject([{ ms: RESOURCE_DEADLINE_MS, cancelled: false }])
  clock.pending[0]!.elapsed()
  expect(await pending).toMatchObject({ code: "timeout", status: undefined })
  expect(requestSignal?.aborted).toBe(true)
})

it("reports the caller's own abort as aborted, and sends nothing when already aborted", async () => {
  const clock = manualTimer()
  const caller = new AbortController()
  let requestSignal: AbortSignal | undefined
  const get = vi.fn((request: { signal?: AbortSignal }) => {
    requestSignal = request.signal
    // What `fetch` does when its signal aborts.
    return new Promise<McpResourceReply>((_, reject) =>
      request.signal?.addEventListener("abort", () =>
        reject(new DOMException("The operation was aborted.", "AbortError")),
      ),
    )
  })
  const pending = failure(
    fetches(get, clock).fetchResource(
      ticket,
      { size: html.byteLength, sha256 },
      { signal: caller.signal },
    ),
  )
  caller.abort()
  expect(await pending).toMatchObject({ code: "aborted" })
  expect(requestSignal?.aborted).toBe(true)
  expect(clock.pending[0]!.cancelled).toBe(true)

  const unsent = vi.fn()
  await expect(
    fetches(unsent).fetchResource(
      ticket,
      { size: html.byteLength, sha256 },
      { signal: caller.signal },
    ),
  ).rejects.toMatchObject({ code: "aborted" })
  expect(unsent).not.toHaveBeenCalled()
})

it.each([
  ["a short ticket", ticket.slice(1), { size: 1, sha256 }],
  ["a long ticket", `${ticket}A`, { size: 1, sha256 }],
  ["a ticket outside base64url", `${ticket.slice(1)}=`, { size: 1, sha256 }],
  ["a negative size", ticket, { size: -1, sha256 }],
  ["a fractional size", ticket, { size: 0.5, sha256 }],
  ["a size past 4 MiB", ticket, { size: 4 * 1024 * 1024 + 1, sha256 }],
  ["an uppercase digest", ticket, { size: 1, sha256: sha256.toUpperCase() }],
  ["a prefixed digest", ticket, { size: 1, sha256: `sha256:${sha256}` }],
] as const)(
  "refuses to fetch with %s before any request",
  async (_name, given, expected) => {
    const get = vi.fn()
    await expect(fetches(get).fetchResource(given, expected)).rejects.toBeInstanceOf(
      TypeError,
    )
    expect(get).not.toHaveBeenCalled()
  },
)

it("fetches a resource of exactly 4 MiB", async () => {
  const bytes = new Uint8Array(4 * 1024 * 1024)
  const digest = createHash("sha256").update(bytes).digest("hex")
  expect(
    (
      await fetches(async () => ({ status: 200, bytes })).fetchResource(ticket, {
        size: bytes.byteLength,
        sha256: digest,
      })
    ).byteLength,
  ).toBe(bytes.byteLength)
})

it("releases one mount of an app and returns the gateway's acknowledgement", async () => {
  const request = vi.fn(async () => ({ requestId: "release", applied: true }))
  expect(
    await api(request).releaseApp(conversationId, app, { requestId: "release" }),
  ).toEqual({ requestId: "release", applied: true })
  // On the control lane, with no deadline of its own: nothing it waits on.
  expect(request).toHaveBeenCalledExactlyOnceWith("mcp.releaseApp", {
    conversationId,
    requestId: "release",
    app,
  })
})

it.each([
  ["another action's acknowledgement", { requestId: "other", applied: true }],
  ["no applied flag", { requestId: "release" }],
  ["an unknown field", { requestId: "release", applied: true, released: 2 }],
])("refuses a release answer that is %s", async (_name, reply) => {
  const error = await failure(
    api(async () => reply).releaseApp(conversationId, app, { requestId: "release" }),
  )
  expect(error).toBeInstanceOf(NessaConversationControlError)
  expect(error).toMatchObject({ uncertain: true, requestId: "release" })
})

it("reports a refused release as certain and a lost one as uncertain", async () => {
  const refused = await failure(
    api(() =>
      Promise.reject(new NessaRpcError("conversation_not_found", "x")),
    ).releaseApp(conversationId, app),
  )
  expect(refused).toBeInstanceOf(NessaConversationControlError)
  expect(refused).toMatchObject({
    code: "conversation_not_found",
    uncertain: false,
    executionId: app.executionId,
  })
  const lost = await failure(
    api(() => Promise.reject(new Error("socket closed"))).releaseApp(conversationId, app),
  )
  expect(lost).toMatchObject({ code: undefined, uncertain: true })
  const request = vi.fn()
  await expect(
    api(request).releaseApp(conversationId, { ...app, instanceId: "" }),
  ).rejects.toBeInstanceOf(TypeError)
  expect(request).not.toHaveBeenCalled()
})

describe("what an app may send its server (mcpAppRequestProblem)", () => {
  it("answers nothing for a request within bounds, and in Unicode", () => {
    expect(mcpAppRequestProblem.tool("é".repeat(64))).toBeUndefined()
    expect(mcpAppRequestProblem.tool("chart\ud83d\udcc8")).toBeUndefined()
    expect(mcpAppRequestProblem.uri("ui://w/app.html")).toBeUndefined()
    expect(
      mcpAppRequestProblem.argumentsJson('{"a":["\\ud83d\\udcc8",{"b":1}]}'),
    ).toBeUndefined()
    // Not JSON: whether the arguments are an object is the gateway's to say.
    expect(mcpAppRequestProblem.argumentsJson("[")).toBeUndefined()
    // Escaped, a lone surrogate is ASCII in the frame: what it decodes to is
    // the gateway's to judge, and it answers `invalid_request`.
    expect(
      mcpAppRequestProblem.argumentsJson(JSON.stringify({ a: "\ud800" })),
    ).toBeUndefined()
  })

  it.each([
    ["tool", ""],
    ["tool", "é".repeat(65)],
    ["tool", "\ud800"],
    ["tool", "a\udc00b"],
    ["uri", ""],
    ["uri", `ui://${"x".repeat(2044)}`],
    ["uri", "ui://w/\udfff"],
    ["argumentsJson", `{"a":"${"x".repeat(32761)}"}`],
    ["argumentsJson", '{"a":"\ud800"}'],
    ["argumentsJson", '{"\udc00":1}'],
  ] as const)("says what is wrong with a %s of %j", (kind, value) => {
    expect(mcpAppRequestProblem[kind](value)).toEqual(expect.any(String))
  })
})

it("refuses to read a URI that is no string, or not Unicode, before asking the gateway", async () => {
  const request = vi.fn()
  for (const uri of [7 as unknown as string, "ui://w/\ud800"])
    await expect(
      api(request).readResource(conversationId, app, "charts", uri),
    ).rejects.toBeInstanceOf(TypeError)
  expect(request).not.toHaveBeenCalled()
})

it("is certain nothing reached the gateway for a request too large to send", async () => {
  const cause = new NessaRequestTooLargeError("mcp.callTool", 70_000)
  const error = await failure(
    api(async () => Promise.reject(cause)).callTool(
      conversationId,
      app,
      "charts",
      "list",
    ),
  )
  expect(error).toBeInstanceOf(NessaMcpAppError)
  expect(error).toMatchObject({ code: undefined, uncertain: false, cause })
})
