/**
 * `McpAppConversation` over a fake `client.mcpApps` (#390): a message is the
 * app's text blocks, sent as its mount; a context its text and structure;
 * the gateway's refusals are `refused`, and anything without a code is the
 * adapter's fault, for the bridge to log. The last test runs the real bridge
 * over this adapter, from the app's `ui/message` to the gateway's call.
 */
import {
  MAX_MCP_CONTEXT_BYTES,
  MAX_MCP_MESSAGE_BYTES,
  mcpAppDeadlines,
  NessaMcpAppError,
  NessaRpcError,
  type McpAppReference,
  type McpAppsApi,
} from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import type { AppAddress } from "../../application/ports"
import { gatewayAppConversation, type AppConversationApi } from "./app-messages"

const conversationId = "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d"
const app: McpAppReference = {
  executionId: "execution-1",
  toolId: "call-1",
  instanceId: "6f1d2c3b-4a5e-4f60-8172-839405a6b7c8",
}
const address: AppAddress = { sessionId: conversationId, server: "weather", app }

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

function refusal(code: string): NessaMcpAppError {
  return new NessaMcpAppError(
    conversationId,
    "request-1",
    app,
    new NessaRpcError(code, `refused: ${code}`),
  )
}

const text = (value: string) => ({ type: "text", text: value })

describe("an app's message", () => {
  it("is its text blocks, a blank line between each, sent as its mount", async () => {
    const apps = fakeApps()
    const conversation = gatewayAppConversation(apps)
    expect(
      await conversation.sendMessage(address, [text("Plot May"), text("next to April")]),
    ).toBe("done")
    expect(apps.sendMessage).toHaveBeenCalledExactlyOnceWith(
      conversationId,
      app,
      "weather",
      "Plot May\n\nnext to April",
    )
    // As long as the client waits for it: a review, then the send.
    expect(conversation.within).toBe(mcpAppDeadlines.sendMessageMs)
    expect(conversation.within).toBe(mcpAppDeadlines.updateModelContextMs)
  })

  it.each(["turn_running", "mcp_approval_denied", "mcp_cancelled", "invalid_request"])(
    "is refused when the gateway refuses it %s",
    async (code) => {
      const apps = fakeApps({
        sendMessage: vi.fn(async () => Promise.reject(refusal(code))),
      })
      expect(await gatewayAppConversation(apps).sendMessage(address, [text("hi")])).toBe(
        "refused",
      )
    },
  )

  it("is refused, unsent, when it has no text or more than the gateway takes", async () => {
    const apps = fakeApps()
    const conversation = gatewayAppConversation(apps)
    expect(await conversation.sendMessage(address, [])).toBe("refused")
    expect(
      await conversation.sendMessage(address, [
        text("x".repeat(MAX_MCP_MESSAGE_BYTES + 1)),
      ]),
    ).toBe("refused")
    expect(apps.sendMessage).not.toHaveBeenCalled()
    // Exactly at the bound is sent.
    await conversation.sendMessage(address, [text("é".repeat(MAX_MCP_MESSAGE_BYTES / 2))])
    expect(apps.sendMessage).toHaveBeenCalledTimes(1)
  })

  it("rejects for what is no answer of the gateway's, for the bridge to log", async () => {
    const lost = new NessaMcpAppError(
      conversationId,
      "request-1",
      app,
      new Error("closed"),
    )
    for (const error of [lost, new Error("socket closed")]) {
      const apps = fakeApps({ sendMessage: vi.fn(async () => Promise.reject(error)) })
      await expect(
        gatewayAppConversation(apps).sendMessage(address, [text("hi")]),
      ).rejects.toBe(error)
    }
  })
})

describe("an app's context", () => {
  it("is its text blocks and its structured content, encoded", async () => {
    const apps = fakeApps()
    expect(
      await gatewayAppConversation(apps).updateModelContext(address, {
        content: [text("Showing April"), text("week 2")],
        structuredContent: { month: 4 },
      }),
    ).toBe("done")
    expect(apps.updateModelContext).toHaveBeenCalledExactlyOnceWith(
      conversationId,
      app,
      "weather",
      { text: "Showing April\n\nweek 2", structuredContentJson: '{"month":4}' },
    )
  })

  it("is cleared by an update with neither part", async () => {
    const apps = fakeApps()
    await gatewayAppConversation(apps).updateModelContext(address, {})
    await gatewayAppConversation(apps).updateModelContext(address, { content: [] })
    expect(apps.updateModelContext.mock.calls.map((call) => call[3])).toEqual([{}, {}])
  })

  it("is refused, unsent, with a part past the wire's bound, and leaves together to the gateway", async () => {
    const apps = fakeApps()
    const conversation = gatewayAppConversation(apps)
    expect(
      await conversation.updateModelContext(address, {
        content: [text("x".repeat(MAX_MCP_CONTEXT_BYTES + 1))],
      }),
    ).toBe("refused")
    expect(apps.updateModelContext).not.toHaveBeenCalled()
    // Each part within it: the gateway decides whether they fit together.
    expect(
      await conversation.updateModelContext(address, {
        content: [text("x".repeat(MAX_MCP_CONTEXT_BYTES))],
        structuredContent: { a: 1 },
      }),
    ).toBe("done")
    expect(apps.updateModelContext).toHaveBeenCalledTimes(1)
  })

  it.each(["temporarily_unavailable", "mcp_request_too_large", "mcp_cancelled"])(
    "is refused when the gateway refuses it %s",
    async (code) => {
      const apps = fakeApps({
        updateModelContext: vi.fn(async () => Promise.reject(refusal(code))),
      })
      expect(
        await gatewayAppConversation(apps).updateModelContext(address, {
          content: [text("x")],
        }),
      ).toBe("refused")
    },
  )
})
