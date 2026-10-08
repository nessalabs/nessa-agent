/** What an app is told of its call, and when (#349, L8, L12, O1, O2). */
import { describe, expect, it } from "vitest"
import {
  nothingTold,
  toolNotifications,
  type CallPhase,
  type CallTold,
} from "./tool-call"

const methods = (told: CallTold, phase: CallPhase) =>
  toolNotifications(told, phase).send.map((message) =>
    "method" in message ? [message.method, message.params] : message,
  )

describe("the order of the spec's notifications", () => {
  it("partials while the input streams, each new one once", () => {
    let told = nothingTold
    const first = toolNotifications(told, { kind: "streaming", partial: { a: 1 } })
    expect(first.send).toEqual([
      {
        jsonrpc: "2.0",
        method: "ui/notifications/tool-input-partial",
        params: { arguments: { a: 1 } },
      },
    ])
    told = first.told
    expect(
      toolNotifications(told, { kind: "streaming", partial: { a: 1 } }).send,
    ).toEqual([])
    expect(methods(told, { kind: "streaming", partial: { a: 1, b: 2 } })).toEqual([
      ["ui/notifications/tool-input-partial", { arguments: { a: 1, b: 2 } }],
    ])
  })

  it("O1: a call that ended before the view was ready is told input, then result, at once", () => {
    expect(
      methods(nothingTold, {
        kind: "done",
        arguments: { location: "San Francisco" },
        result: {
          content: [{ type: "text", text: "Sunny" }],
          structuredContent: { t: 72 },
        },
      }),
    ).toEqual([
      ["ui/notifications/tool-input", { arguments: { location: "San Francisco" } }],
      [
        "ui/notifications/tool-result",
        { content: [{ type: "text", text: "Sunny" }], structuredContent: { t: 72 } },
      ],
    ])
  })

  it("a running call with no arguments yet tells the app nothing; the real ones come before the result", () => {
    const running = toolNotifications(nothingTold, { kind: "running" })
    expect(running.send).toEqual([])
    expect(
      methods(running.told, {
        kind: "done",
        arguments: { city: "Oslo" },
        result: { content: [] },
      }),
    ).toEqual([
      ["ui/notifications/tool-input", { arguments: { city: "Oslo" } }],
      ["ui/notifications/tool-result", { content: [] }],
    ])
  })

  it("O2: no partial once the input is complete; input and result once each", () => {
    const running = toolNotifications(nothingTold, {
      kind: "running",
      arguments: { a: 1 },
    })
    expect(running.send).toHaveLength(1)
    expect(
      toolNotifications(running.told, { kind: "streaming", partial: { a: 2 } }).send,
    ).toEqual([])
    expect(
      toolNotifications(running.told, { kind: "running", arguments: { a: 1 } }).send,
    ).toEqual([])
    const done = toolNotifications(running.told, {
      kind: "done",
      arguments: { a: 1 },
      result: { content: [] },
    })
    expect(done.send.map((m) => ("method" in m ? m.method : ""))).toEqual([
      "ui/notifications/tool-result",
    ])
    expect(
      toolNotifications(done.told, { kind: "done", arguments: { a: 1 }, result: {} })
        .send,
    ).toEqual([])
    expect(toolNotifications(done.told, { kind: "cancelled" }).send).toEqual([])
  })

  it("cancelled once, whatever was sent, with its reason when it has one", () => {
    expect(methods(nothingTold, { kind: "cancelled", reason: "user action" })).toEqual([
      ["ui/notifications/tool-cancelled", { reason: "user action" }],
    ])
    const streaming = toolNotifications(nothingTold, { kind: "streaming", partial: {} })
    expect(methods(streaming.told, { kind: "cancelled" })).toEqual([
      ["ui/notifications/tool-cancelled", {}],
    ])
    expect(methods(nothingTold, { kind: "cancelled", arguments: { a: 1 } })).toEqual([
      ["ui/notifications/tool-input", { arguments: { a: 1 } }],
      ["ui/notifications/tool-cancelled", {}],
    ])
  })
})
