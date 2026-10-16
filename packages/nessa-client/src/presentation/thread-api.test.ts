import { describe, expect, it, vi } from "vitest"
import { createConversationApi } from "./conversation-api.js"
import { createThreadCommands } from "./thread-api.js"
import { NessaConversationMutationError } from "../application/conversation-mutation-error.js"
import type { ConversationView } from "../generated/product.js"

const id = "00000000-0000-4000-8000-000000000001"

function snapshot(running?: string): ConversationView {
  return {
    conversationId: id,
    artifacts: [],
    notes: [],
    unreadable: [],
    unreadableTruncated: false,
    approvalMode: "ask",
    approvalModes: [{ id: "ask", name: "Ask", description: "Ask" }],
    title: null,
    revision: "one",
    serverNowMs: 1,
    messages: [
      ...(running
        ? [
            {
              executionId: running,
              userText: "hello",
              attachments: [],
              files: [],
              status: "running" as const,
              parts: [],
            },
          ]
        : []),
      {
        executionId: "queued",
        userText: "later",
        attachments: [],
        files: [],
        status: "queued",
        parts: [],
      },
    ],
    pending: [
      {
        executionId: "queued",
        text: "later",
        attachments: [],
        files: [],
        mode: "queued",
      },
    ],
    permissions: [],
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
        nativeHooks: "off",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unsupported_not_implemented",
        policyEndTurn: "unsupported",
        policyCloseSession: "unsupported",
        incomingElicitation: "unsupported",
      },
    },
    lifecycle: { phase: "absent" },
    truncated: false,
    queueComplete: true,
    transcriptState: "complete",
  }
}

function commands(reply: (method: string, params: Record<string, unknown>) => unknown) {
  const request = vi.fn(async (method: string, params: unknown) =>
    reply(method, params as Record<string, unknown>),
  )
  let count = 0
  const newId = () => `request-${++count}`
  const conversation = createConversationApi({ request }, newId)
  return { api: createThreadCommands(conversation, newId, { request }), request }
}

describe("thread commands", () => {
  it("creates through the existing owner and keeps original creation defaults", async () => {
    const { api, request } = commands((_method, params) => ({
      conversationId: params.conversationId,
    }))
    expect(await api.createThread({ conversationId: id })).toEqual({ conversationId: id })
    expect(request).toHaveBeenCalledExactlyOnceWith("conversation.create", {
      conversationId: id,
      requestId: "request-1",
    })
  })

  it("reuses the caller request ID as the execution identity on explicit repeated sends", async () => {
    const { api, request } = commands((_method, params) => ({
      executionId: params.executionId,
      disposition: "queued",
    }))
    for (let count = 0; count < 2; count++)
      await api.sendMessage(id, "hello", { requestId: "same" })
    expect(request.mock.calls[0]).toEqual(request.mock.calls[1])
    expect(request.mock.calls[0]).toEqual([
      "conversation.send",
      {
        conversationId: id,
        text: "hello",
        executionId: "same",
        requestId: "same",
        attachments: [],
        files: [],
      },
    ])
  })

  it("keeps provider queue admission truthful for a steering request", async () => {
    const { api, request } = commands((_method, params) => ({
      executionId: params.executionId,
      disposition: "queued",
    }))
    expect(await api.sendMessage(id, "hello", { mode: "steering" })).toMatchObject({
      disposition: "queued",
    })
    expect(request.mock.calls[0][0]).toBe("conversation.steer")
  })

  it("records passive text without scheduling model delivery", async () => {
    const receipt = {
      requestId: "note",
      messageId: "message",
      acceptedAtMs: 1,
      deliveryState: "recorded",
    }
    const { api, request } = commands(() => receipt)
    expect(
      await api.sendMessage(id, "a note", { mode: "passive", requestId: "note" }),
    ).toEqual(receipt)
    expect(request).toHaveBeenCalledExactlyOnceWith("conversation.message", {
      conversationId: id,
      requestId: "note",
      text: "a note",
      delivery: "record_only",
    })
  })

  it("keeps immutable original inputs and IDs in existing explicit retry", async () => {
    let calls = 0
    const { api, request } = commands((_method, params) => {
      if (++calls === 1) throw new Error("lost reply")
      return { executionId: params.executionId, disposition: "settled" }
    })
    const options = { requestId: "original" }
    const pending = api.sendMessage(id, "original text", options)
    options.requestId = "changed"
    const error = await pending.catch((error: unknown) => error)
    expect(error).toBeInstanceOf(NessaConversationMutationError)
    await (error as NessaConversationMutationError<unknown>).retry()
    expect(request.mock.calls[0]).toEqual(request.mock.calls[1])
  })

  it("stops explicit executions without reading or closing the thread", async () => {
    const { api, request } = commands((_method, params) => ({
      requestId: params.requestId,
      stage: "settled",
      outcome: "already_final",
    }))
    expect(await api.stopThread(id, { executionId: "old" })).toMatchObject({
      outcome: "already_final",
    })
    expect(request).toHaveBeenCalledExactlyOnceWith("conversation.stop", {
      conversationId: id,
      executionId: "old",
      requestId: "request-1",
    })
  })

  it("captures A through passive peek once and does not substitute B after A finishes", async () => {
    const state = snapshot("A")
    const { api, request } = commands((method, params) => {
      if (method === "conversation.peek") return state
      // A new turn can already run when the stop reaches the gateway.
      state.messages[0].executionId = "B"
      expect(params.executionId).toBe("A")
      return { requestId: params.requestId, stage: "settled", outcome: "already_final" }
    })
    expect(await api.stopThread(id)).toMatchObject({ outcome: "already_final" })
    expect(request.mock.calls.map(([method]) => method)).toEqual([
      "conversation.peek",
      "conversation.stop",
    ])
  })

  it("returns no_active_turn from an idle peek without stopping queued work", async () => {
    const { api, request } = commands(() => snapshot())
    expect(await api.stopThread(id)).toEqual({ outcome: "no_active_turn" })
    expect(request.mock.calls.map(([method]) => method)).toEqual(["conversation.peek"])
  })

  it("rejects an inconsistent peek identity before any stop", async () => {
    const { api, request } = commands(() => ({
      ...snapshot("A"),
      conversationId: "00000000-0000-4000-8000-000000000002",
    }))
    await expect(api.stopThread(id)).rejects.toThrow()
    expect(request).toHaveBeenCalledTimes(1)
  })

  it("rejects an unknown mode before sending anything", () => {
    const { api, request } = commands(() => undefined)
    expect(() => api.sendMessage(id, "hello", { mode: "other" as never })).toThrow(
      TypeError,
    )
    expect(request).not.toHaveBeenCalled()
  })
})
