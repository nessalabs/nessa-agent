// @vitest-environment jsdom
/** Native setting receipts and independent source frames remain separate. */
import { describe, expect, it } from "vitest"
import { NessaConversationControlError, NessaRpcError } from "@nessa/client"
import { fakeGateway, row, view } from "./fake-gateway"
import { gatewaySource, type GatewayClock } from "./gateway-source"
import type { WorkspaceUpdate } from "../../application/ports"

const old = "claude-opus-5"
const next = { provider: "anthropic", modelId: "claude-sonnet-5" }
const runtime = {
  model: old,
  provider: "test",
  workspace: "/",
  agent: "test",
  modelName: "test",
  contextWindowTokens: 1,
  reasoning: false,
}
const clock: GatewayClock = { now: () => 1_000_000, after: () => () => {} }
async function ready() {
  const gateway = fakeGateway()
  gateway.rows.set("a", row("a"))
  gateway.views.set("a", view("a", { runtime }))
  const source = gatewaySource({
    connect: async () => gateway.client,
    clock,
    timing: { callMs: 5000, retryMs: 1000 },
  })
  const updates: WorkspaceUpdate[] = []
  source.subscribe((update) => updates.push(update))
  await source.index()
  await source.transcript("a")
  return { gateway, source, updates }
}

describe("same-provider gateway control", () => {
  it("preserves request identity and reads for recovery without publishing an ACK as a frame", async () => {
    const { gateway, source, updates } = await ready()
    gateway.once("setModel", () =>
      Promise.resolve({
        requestId: "original-request",
        model: next.modelId,
        approvalMode: "ask",
        effect: "nextTurn",
      }),
    )
    const before = updates.length
    const result = await source.setModel("a", next, "original-request", "person")
    expect(result).toMatchObject({
      requestId: "original-request",
      model: next.modelId,
      effect: "nextTurn",
    })
    expect(gateway.calls.filter((call) => call.method === "setModel")).toEqual([
      {
        method: "setModel",
        args: ["a", next.modelId, { requestId: "original-request" }],
      },
    ])
    expect(gateway.count("read")).toBe(2)
    expect(
      updates.slice(before).filter((update) => update.kind === "transcript"),
    ).toEqual([])
    gateway.publish("a")
    for (let i = 0; i < 20; i++) await Promise.resolve()
    expect((await source.transcript("a")).sessionId).toBe("a")
    source.dispose()
  })

  it("refuses cross-provider before a native setting", async () => {
    const { gateway, source } = await ready()
    await expect(
      source.setModel(
        "a",
        { provider: "openai", modelId: "gpt-6-astra" },
        "id",
        "person",
      ),
    ).rejects.toMatchObject({ reason: "not-supported" })
    expect(gateway.count("setModel")).toBe(0)
    source.dispose()
  })

  it("keeps the primary typed not-applied refusal and does not retry the native operation", async () => {
    const { gateway, source } = await ready()
    gateway.once("setModel", () =>
      Promise.reject(
        new NessaConversationControlError(
          "a",
          "request",
          undefined,
          new NessaRpcError("model_change_not_applied", "not parsed"),
        ),
      ),
    )
    await expect(source.setModel("a", next, "request", "person")).rejects.toMatchObject({
      diagnostic: { code: "model_change_not_applied" },
    })
    expect(gateway.count("setModel")).toBe(1)
    expect(gateway.count("read")).toBe(2)
    source.dispose()
  })

  it("does not dispatch a native setting when the preceding reconciliation read fails", async () => {
    const { gateway, source } = await ready()
    gateway.once("read", () =>
      Promise.reject(new NessaRpcError("conversation_not_found", "not parsed")),
    )
    await expect(source.setModel("a", next, "request", "person")).rejects.toBeDefined()
    expect(gateway.count("setModel")).toBe(0)
    source.dispose()
  })
  it("preserves an uncertain control when its final reconciliation read also fails", async () => {
    const { gateway, source } = await ready()
    gateway.once("setModel", () => {
      gateway.once("read", () =>
        Promise.reject(new NessaRpcError("unavailable", "read unavailable")),
      )
      return Promise.reject(
        new NessaConversationControlError(
          "a",
          "request",
          undefined,
          new NessaRpcError("model_change_uncertain", "not parsed"),
        ),
      )
    })
    await expect(source.setModel("a", next, "request", "person")).rejects.toMatchObject({
      uncertain: true,
      diagnostic: { code: "model_change_uncertain" },
    })
    expect(gateway.count("setModel")).toBe(1)
    source.dispose()
  })
  it("does not ask native while an authoritative reconciliation still requires recovery", async () => {
    const { gateway, source } = await ready()
    gateway.views.set(
      "a",
      view("a", {
        runtime,
        modelChange: {
          requestId: "prior",
          requestedModel: next.modelId,
          status: "recovery_required",
        },
      }),
    )
    await expect(source.setModel("a", next, "request", "person")).rejects.toMatchObject({
      uncertain: true,
    })
    expect(gateway.count("setModel")).toBe(0)
    source.dispose()
  })
  it("reports a reconciled receipt only after the fresh source frame, including a newer controller model", async () => {
    const { gateway, source } = await ready()
    const other = "claude-fable-5-1"
    gateway.once("setModel", () => {
      gateway.views.set("a", view("a", { runtime: { ...runtime, model: other } }))
      return Promise.resolve({
        requestId: "request",
        model: next.modelId,
        approvalMode: "ask",
        effect: "nextTurn",
      })
    })
    await expect(source.setModel("a", next, "request", "person")).resolves.toMatchObject({
      model: next.modelId,
      sourceReconciled: true,
    })
    expect(
      (await source.index()).sessions.find((session) => session.id === "a")?.model
        .modelId,
    ).toBe(other)
    expect(gateway.count("read")).toBe(2)
    source.dispose()
  })
  it("does not call a receipt reconciled when its fresh source read fails", async () => {
    const { gateway, source } = await ready()
    gateway.once("setModel", () => {
      gateway.once("read", () =>
        Promise.reject(new NessaRpcError("unavailable", "read unavailable")),
      )
      return Promise.resolve({
        requestId: "request",
        model: next.modelId,
        approvalMode: "ask",
        effect: "nextTurn",
      })
    })
    await expect(source.setModel("a", next, "request", "person")).resolves.toMatchObject({
      sourceReconciled: false,
    })
    source.dispose()
  })
  it("does not call a fresh frame reconciled when it has no known model", async () => {
    const { gateway, source } = await ready()
    gateway.once("setModel", () => {
      gateway.views.set("a", view("a", { runtime: undefined }))
      return Promise.resolve({
        requestId: "request",
        model: next.modelId,
        approvalMode: "ask",
        effect: "nextTurn",
      })
    })
    await expect(source.setModel("a", next, "request", "person")).resolves.toMatchObject({
      sourceReconciled: false,
    })
    source.dispose()
  })
  it("keeps recovery required when the current source model is unknown, before any native control", async () => {
    const { gateway, source } = await ready()
    gateway.views.set("a", view("a", { runtime: undefined }))
    gateway.publish("a")
    for (let i = 0; i < 20; i++) await Promise.resolve()
    await expect(source.setModel("a", next, "request", "person")).rejects.toMatchObject({
      reason: "unavailable",
      uncertain: true,
    })
    expect(gateway.count("setModel")).toBe(0)
    source.dispose()
  })
})
