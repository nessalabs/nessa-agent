/**
 * `McpAppConversation` over a fake `client.mcpApps` (#390), each test named
 * after the row of the desktop's table on #390 it holds: a message is the
 * app's text blocks, sent as its mount; a context its text and structure,
 * or neither, which clears; what is outside the client's bounds is `invalid`
 * and unsent; what the gateway answers with a code is read through the same
 * `outcomes` table as the server port's calls; and one mount's updates go one
 * after another.
 */
import {
  ConversationErrorCode,
  MAX_MCP_CONTEXT_BYTES,
  MAX_MCP_MESSAGE_BYTES,
  mcpAppDeadlines,
  mcpAppRequestProblem,
  NessaConnectionClosedError,
  NessaMcpAppError,
  NessaRpcError,
  type McpAppReference,
  type McpAppsApi,
} from "@nessa/client"
import { afterEach, describe, expect, it, vi } from "vitest"
import type { AppAddress } from "../../application/ports"
import { gatewayAppConversation, type AppConversationApi } from "./app-messages"
import { gatewayAppServer } from "./mcp-app-server"

const conversationId = "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d"
const app: McpAppReference = {
  executionId: "execution-1",
  toolId: "call-1",
  instanceId: "6f1d2c3b-4a5e-4f60-8172-839405a6b7c8",
}
const address: AppAddress = { sessionId: conversationId, server: "weather", app }
const otherMount: AppAddress = {
  ...address,
  app: { ...app, instanceId: "0d6c3e7a-1b2c-4d5e-8f90-a1b2c3d4e5f6" },
}

function fakeApps(overrides: Partial<AppConversationApi> = {}) {
  const apps = {
    sendMessage: vi.fn<McpAppsApi["sendMessage"]>(async () => ({ executionId: "turn" })),
    updateModelContext: vi.fn<McpAppsApi["updateModelContext"]>(async () => ({
      requestId: "request-1",
      applied: true,
    })),
  }
  return Object.assign(apps, overrides)
}

/** The error `client.mcpApps` throws when the gateway refused the call with `code`. */
function refusal(code: string): NessaMcpAppError {
  return new NessaMcpAppError(
    conversationId,
    "request-1",
    app,
    new NessaRpcError(code, `refused: ${code}`),
  )
}

const text = (value: string) => ({ type: "text", text: value })
const taken = { kind: "ok", result: {} }
/** The signal of a request the bridge has not answered yet, its mount live. */
const live = new AbortController().signal
const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

afterEach(() => {
  vi.restoreAllMocks()
})

describe("an app's message", () => {
  it("D1: is its text blocks, a blank line between each, sent as its mount, and taken", async () => {
    const apps = fakeApps()
    expect(
      await gatewayAppConversation(apps).sendMessage(address, [
        text("Plot May"),
        text(""),
        text("next to April"),
      ]),
    ).toEqual(taken)
    expect(apps.sendMessage).toHaveBeenCalledExactlyOnceWith(
      conversationId,
      app,
      "weather",
      "Plot May\n\nnext to April",
    )
  })

  it("D3: is invalid and unsent when its text is empty after the join, past its bound, or no Unicode text, in the client's words", async () => {
    const apps = fakeApps()
    const conversation = gatewayAppConversation(apps)
    for (const content of [
      [],
      [text("")],
      [text(""), text("")],
      [text("x".repeat(MAX_MCP_MESSAGE_BYTES + 1))],
      [text("\uD800")],
    ]) {
      const joined = content
        .map((block) => block.text)
        .filter(Boolean)
        .join("\n\n")
      expect(await conversation.sendMessage(address, content)).toEqual({
        kind: "invalid",
        reason: mcpAppRequestProblem.message(joined),
      })
    }
    expect(apps.sendMessage).not.toHaveBeenCalled()
    // Exactly at the bound, in UTF-8 bytes, is sent.
    expect(
      await conversation.sendMessage(address, [
        text("é".repeat(MAX_MCP_MESSAGE_BYTES / 2)),
      ]),
    ).toEqual(taken)
    expect(apps.sendMessage).toHaveBeenCalledTimes(1)
  })

  it.each([
    ["turn_running", "The conversation is busy"],
    ["mcp_approval_denied", "The person declined this action"],
    ["mcp_approval_expired", "No one answered in time"],
    ["mcp_cancelled", "The request was withdrawn"],
    ["invalid_request", "The gateway refused the request as invalid"],
    ["mcp_request_too_large", "The request is larger than the gateway accepts"],
    ["mcp_app_unknown", "This app may not speak in this conversation"],
    ["mcp_server_mismatch", "This app may not speak in this conversation"],
  ])("D4: is refused when the gateway refuses it %s", async (code, reason) => {
    const apps = fakeApps({ sendMessage: vi.fn(() => Promise.reject(refusal(code))) })
    expect(await gatewayAppConversation(apps).sendMessage(address, [text("hi")])).toEqual(
      {
        kind: "refused",
        reason,
      },
    )
  })

  it("D5: is uncertain, not busy, for temporarily_unavailable: the client cannot say the agent does not have it", async () => {
    const error = refusal("temporarily_unavailable")
    expect(error.uncertain).toBe(true)
    const apps = fakeApps({ sendMessage: vi.fn(() => Promise.reject(error)) })
    expect(await gatewayAppConversation(apps).sendMessage(address, [text("hi")])).toEqual(
      { kind: "uncertain", serverGone: false },
    )
  })

  it.each(["conversation_closed", "mcp_session_unavailable"])(
    "D6: is uncertain, with the server gone, for %s: the turn may have been taken before it went",
    async (code) => {
      const error = refusal(code)
      expect(error.uncertain).toBe(true)
      const apps = fakeApps({ sendMessage: vi.fn(() => Promise.reject(error)) })
      expect(
        await gatewayAppConversation(apps).sendMessage(address, [text("hi")]),
      ).toEqual({ kind: "uncertain", serverGone: true })
    },
  )

  it.each(["conversation_not_found", "conversation_deleted"])(
    "D6: finds the server gone, certainly, when the gateway answers %s before anything was taken",
    async (code) => {
      const error = refusal(code)
      expect(error.uncertain).toBe(false)
      const apps = fakeApps({ sendMessage: vi.fn(() => Promise.reject(error)) })
      expect(
        await gatewayAppConversation(apps).sendMessage(address, [text("hi")]),
      ).toEqual({ kind: "server-gone" })
    },
  )

  it("D7: fails, logged, for no code, an answer the client did not believe, or a fault", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {})
    const lost = new NessaMcpAppError(
      conversationId,
      "request-1",
      app,
      new NessaConnectionClosedError(1006, ""),
    )
    const unbelieved = new NessaMcpAppError(
      conversationId,
      "request-1",
      app,
      new Error("Invalid message executionId"),
    )
    for (const thrown of [lost, unbelieved, new Error("fault"), "a string"]) {
      const apps = fakeApps({ sendMessage: vi.fn(() => Promise.reject(thrown)) })
      expect(
        await gatewayAppConversation(apps).sendMessage(address, [text("hi")]),
      ).toEqual({ kind: "failed" })
    }
    expect(error).toHaveBeenCalledTimes(4)
  })

  it("D-B: asks the client's certainty first, then reads every gateway code through the server port's one table: a certain code is the same outcome whatever the app asked, an uncertain one never a refusal", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {})
    for (const code of Object.values(ConversationErrorCode)) {
      const refused = () => Promise.reject(refusal(code))
      const message = await gatewayAppConversation(
        fakeApps({ sendMessage: vi.fn(refused) }),
      ).sendMessage(address, [text("hi")])
      const context = await gatewayAppConversation(
        fakeApps({ updateModelContext: vi.fn(refused) }),
      ).updateModelContext(address, {}, live)
      const call = await gatewayAppServer(
        { callTool: vi.fn(refused) } as unknown as McpAppsApi,
        () => () => {},
        () => "release",
      ).callTool(address, "t", {})
      const expected =
        refusal(code).uncertain && call.kind !== "failed"
          ? { kind: "uncertain", serverGone: call.kind === "server-gone" }
          : call
      // The same outcome; its words are by what was asked.
      const kindOf = ({ kind, ...rest }: { kind: string; serverGone?: boolean }) =>
        "serverGone" in rest ? { kind, serverGone: rest.serverGone } : { kind }
      expect(kindOf(message), code).toEqual(kindOf(expected))
      expect(kindOf(context), code).toEqual(kindOf(expected))
    }
  })
})

describe("an app's context", () => {
  it("D11: is its text blocks and its structured content, encoded, sent as its mount", async () => {
    const apps = fakeApps()
    expect(
      await gatewayAppConversation(apps).updateModelContext(
        address,
        {
          content: [text("Showing April"), text(""), text("week 2")],
          structuredContent: { month: 4 },
        },
        live,
      ),
    ).toEqual(taken)
    expect(apps.updateModelContext).toHaveBeenCalledExactlyOnceWith(
      conversationId,
      app,
      "weather",
      { text: "Showing April\n\nweek 2", structuredContentJson: '{"month":4}' },
    )
  })

  it("D12: is sent with neither part — a clear — for {}, no blocks, or blocks that say nothing", async () => {
    const apps = fakeApps()
    const conversation = gatewayAppConversation(apps)
    for (const context of [{}, { content: [] }, { content: [text(""), text("")] }])
      expect(await conversation.updateModelContext(address, context, live)).toEqual(taken)
    expect(apps.updateModelContext.mock.calls.map((call) => call[3])).toEqual([
      {},
      {},
      {},
    ])
  })

  it("D13: is invalid and unsent with a part past its bound or no Unicode text; each part within it goes, both together the gateway's to judge", async () => {
    const apps = fakeApps()
    const conversation = gatewayAppConversation(apps)
    const past = "x".repeat(MAX_MCP_CONTEXT_BYTES + 1)
    for (const [context, update] of [
      [{ content: [text(past)] }, { text: past }],
      [{ content: [text("\uDC00")] }, { text: "\uDC00" }],
      [
        { structuredContent: { big: "y".repeat(MAX_MCP_CONTEXT_BYTES) } },
        {
          structuredContentJson: JSON.stringify({
            big: "y".repeat(MAX_MCP_CONTEXT_BYTES),
          }),
        },
      ],
    ] as const)
      expect(await conversation.updateModelContext(address, context, live)).toEqual({
        kind: "invalid",
        reason: mcpAppRequestProblem.context(update),
      })
    expect(apps.updateModelContext).not.toHaveBeenCalled()
    expect(
      await conversation.updateModelContext(
        address,
        {
          content: [text("x".repeat(MAX_MCP_CONTEXT_BYTES))],
          structuredContent: { a: 1 },
        },
        live,
      ),
    ).toEqual(taken)
    expect(apps.updateModelContext).toHaveBeenCalledTimes(1)
  })

  it.each([
    [
      "mcp_request_too_large",
      { kind: "refused", reason: "The request is larger than the gateway accepts" },
    ],
    [
      "invalid_request",
      { kind: "refused", reason: "The gateway refused the request as invalid" },
    ],
    ["mcp_cancelled", { kind: "refused", reason: "The request was withdrawn" }],
    [
      "mcp_app_unknown",
      { kind: "refused", reason: "This app may not speak in this conversation" },
    ],
    ["conversation_not_found", { kind: "server-gone" }],
  ])(
    "D14: is answered for %s as a tool's call would be: the client is certain",
    async (code, answer) => {
      const apps = fakeApps({
        updateModelContext: vi.fn(() => Promise.reject(refusal(code))),
      })
      expect(
        await gatewayAppConversation(apps).updateModelContext(
          address,
          { content: [text("x")] },
          live,
        ),
      ).toEqual(answer)
    },
  )

  it.each([
    ["temporarily_unavailable", { kind: "uncertain", serverGone: false }],
    ["conversation_closed", { kind: "uncertain", serverGone: true }],
    ["mcp_session_unavailable", { kind: "uncertain", serverGone: true }],
  ])(
    "D14: is uncertain for %s, never a refusal: the client cannot say it was not held",
    async (code, answer) => {
      const apps = fakeApps({
        updateModelContext: vi.fn(() => Promise.reject(refusal(code))),
      })
      expect(
        await gatewayAppConversation(apps).updateModelContext(
          address,
          { content: [text("x")] },
          live,
        ),
      ).toEqual(answer)
    },
  )
})

describe("a mount's context updates (D15)", () => {
  it("D15: are sent one after another, in the order the app gave them; another mount's go at once", async () => {
    const sent: string[] = []
    const answers: (() => void)[] = []
    const apps = fakeApps({
      updateModelContext: vi.fn(async (_conversation, _app, _server, context) => {
        sent.push(context.text ?? "(clear)")
        await new Promise<void>((resolve) => answers.push(resolve))
        return { requestId: "request-1", applied: true }
      }),
    })
    const conversation = gatewayAppConversation(apps)
    const first = conversation.updateModelContext(
      address,
      { content: [text("first")] },
      live,
    )
    const second = conversation.updateModelContext(
      address,
      { content: [text("second")] },
      live,
    )
    const clear = conversation.updateModelContext(address, {}, live)
    const other = conversation.updateModelContext(
      otherMount,
      { content: [text("other mount")] },
      live,
    )
    await new Promise((resolve) => setTimeout(resolve, 0))
    // The second waits for the first; another mount's does not.
    expect(sent).toEqual(["first", "other mount"])
    answers.shift()?.()
    expect(await first).toEqual(taken)
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(sent).toEqual(["first", "other mount", "second"])
    answers.shift()?.()
    answers.shift()?.()
    expect(await other).toEqual(taken)
    expect(await second).toEqual(taken)
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(sent).toEqual(["first", "other mount", "second", "(clear)"])
    answers.shift()?.()
    expect(await clear).toEqual(taken)
  })

  it("D15: go on after one the gateway refused, or that failed", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {})
    let calls = 0
    const apps = fakeApps({
      updateModelContext: vi.fn(async () => {
        calls += 1
        if (calls === 1) throw refusal("temporarily_unavailable")
        if (calls === 2) throw new Error("fault")
        return { requestId: "request-1", applied: true }
      }),
    })
    const conversation = gatewayAppConversation(apps)
    const first = conversation.updateModelContext(address, { content: [text("a")] }, live)
    const second = conversation.updateModelContext(
      address,
      { content: [text("b")] },
      live,
    )
    const third = conversation.updateModelContext(address, { content: [text("c")] }, live)
    expect(await first).toEqual({ kind: "uncertain", serverGone: false })
    expect(await second).toEqual({ kind: "failed" })
    expect(await third).toEqual(taken)
  })

  it("D15, D-D (E2-3): a mount's queue keeps nothing once it drains, whatever its updates answered", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {})
    // The adapter's own table of queues, as the first update for this mount
    // is set in it.
    const set = vi.spyOn(Map.prototype, "set")
    let calls = 0
    const apps = fakeApps({
      updateModelContext: vi.fn(async () => {
        calls += 1
        if (calls === 2) throw refusal("temporarily_unavailable")
        if (calls === 3) throw new Error("fault")
        return { requestId: "request-1", applied: true }
      }),
    })
    const conversation = gatewayAppConversation(apps)
    const over = new AbortController()
    over.abort()
    const updates = [
      conversation.updateModelContext(address, { content: [text("a")] }, live),
      conversation.updateModelContext(address, { content: [text("b")] }, live),
      conversation.updateModelContext(address, { content: [text("c")] }, live),
      conversation.updateModelContext(address, { content: [text("d")] }, over.signal),
      conversation.updateModelContext(otherMount, { content: [text("e")] }, live),
    ]
    const queues = set.mock.calls.flatMap((args, index) =>
      args[0] === app.instanceId
        ? [set.mock.contexts[index] as Map<string, unknown>]
        : [],
    )
    set.mockRestore()
    // One table, keyed by mount.
    expect(new Set(queues).size).toBe(1)
    const [queue] = queues
    expect(queue?.has(app.instanceId)).toBe(true)
    await Promise.all(updates)
    await flush()
    expect(queue?.size).toBe(0)
  })
})

describe("a context update whose request is over (D15, D-D)", () => {
  it("is never sent once its signal is aborted before its turn, and the next goes in its place, in order", async () => {
    const sent: string[] = []
    const answers: (() => void)[] = []
    const apps = fakeApps({
      updateModelContext: vi.fn(async (_conversation, _app, _server, context) => {
        sent.push(context.text ?? "(clear)")
        await new Promise<void>((resolve) => answers.push(resolve))
        return { requestId: "request-1", applied: true }
      }),
    })
    const conversation = gatewayAppConversation(apps)
    const over = new AbortController()
    const first = conversation.updateModelContext(address, { content: [text("A")] }, live)
    const dropped = conversation.updateModelContext(
      address,
      { content: [text("B")] },
      over.signal,
    )
    const next = conversation.updateModelContext(address, { content: [text("C")] }, live)
    await flush()
    over.abort()
    answers.shift()?.()
    expect(await first).toEqual(taken)
    // Unsent; no one reads it, as the bridge has answered it already.
    expect(await dropped).toEqual({ kind: "failed" })
    await flush()
    expect(sent).toEqual(["A", "C"])
    answers.shift()?.()
    expect(await next).toEqual(taken)
  })
})

describe("how long either is waited for", () => {
  it("D-F: a call's deadline, as the client waits for both", () => {
    expect(gatewayAppConversation(fakeApps()).within).toBe(mcpAppDeadlines.callToolMs)
  })
})
