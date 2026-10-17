// @vitest-environment jsdom
/**
 * The gateway source's access: the composer shield's choice reaches the
 * gateway (`conversation.create`'s `approvalMode`, `conversation.setApprovalMode`),
 * and the conversation's committed mode is what the transcript says.
 */
import { NessaRpcError } from "@nessa/client"
import { describe, expect, it } from "vitest"
import type { WorkspaceUpdate } from "../../application/ports"
import { fakeGateway, gatewayConversationIds, view } from "./fake-gateway"
import { gatewaySource, type GatewayClock } from "./gateway-source"

const { s: conversationS } = gatewayConversationIds
const timing = { callMs: 5_000, retryMs: 1_000 }
const clock: GatewayClock = { now: () => 1_000_000, after: () => () => {} }

const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve()
}

function started() {
  const gateway = fakeGateway()
  const source = gatewaySource({
    connect: () => Promise.resolve(gateway.client),
    clock,
    timing,
  })
  const updates: WorkspaceUpdate[] = []
  source.subscribe((update) => updates.push(update))
  return { gateway, source, updates }
}

const lastAccess = (updates: readonly WorkspaceUpdate[], id: string) =>
  updates
    .flatMap((update) =>
      update.kind === "transcript" && update.transcript.sessionId === id
        ? [update.transcript.access]
        : [],
    )
    .at(-1)

describe("access", () => {
  it("opens a new conversation at the access chosen before its first message", async () => {
    const { gateway, source } = started()
    await source.send({
      sessionId: conversationS,
      messageId: "m",
      text: "Hello",
      model: { provider: "anthropic", modelId: "claude-opus-5" },
      initiator: "person",
      start: { channelId: "gateway-conversations", title: "Hello", access: "full" },
    })
    const create = gateway.calls.find((call) => call.method === "create")
    expect(create?.args[0]).toMatchObject({
      conversationId: conversationS,
      approvalMode: "full",
    })
  })

  it("asks the gateway to change a begun conversation, and the transcript says what it committed", async () => {
    const { gateway, source, updates } = started()
    gateway.views.set("a", view("a"))
    expect((await source.transcript("a")).access).toEqual({ mode: "ask", modes: ["ask"] })
    await source.setAccess("a", "full", "person")
    expect(
      gateway.calls
        .filter((call) => call.method === "setApprovalMode")
        .map((call) => call.args.slice(0, 2)),
    ).toEqual([["a", "full"]])
    gateway.publish("a")
    await flush()
    expect(lastAccess(updates, "a")?.mode).toBe("full")
  })

  it("says a change the gateway has under way apart from the committed mode", async () => {
    const { gateway, source } = started()
    gateway.views.set(
      "a",
      view("a", {
        approvalMode: "ask",
        approvalModeChange: {
          requestId: "r",
          requestedMode: "full",
          status: "recovery_required",
        },
      }),
    )
    expect((await source.transcript("a")).access).toMatchObject({
      mode: "ask",
      changing: "full",
    })
  })

  it("refuses, typed, a change the gateway refuses while a turn runs", async () => {
    const { gateway, source } = started()
    gateway.views.set("a", view("a"))
    gateway.once("setApprovalMode", () =>
      Promise.reject(new NessaRpcError("turn_running", "A turn is running")),
    )
    await expect(source.setAccess("a", "full", "person")).rejects.toMatchObject({
      reason: "unavailable",
      diagnostic: { code: "turn_running" },
    })
  })
})
