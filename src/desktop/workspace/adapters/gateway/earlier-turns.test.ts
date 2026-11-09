import type { ConversationEarlier, ConversationMessage } from "@nessa/client"
import { describe, expect, it } from "vitest"
import { view } from "./fake-gateway"
import {
  gapOf,
  joined,
  leading,
  startOf,
  withEarlier,
  withGap,
  withPage,
} from "./earlier-turns"

const conversationId = "00000000-0000-4000-8000-000000000001"

const turn = (
  id: string,
  texts: readonly string[],
  change: Partial<ConversationMessage> = {},
): ConversationMessage => ({
  executionId: id,
  userText: `asked ${id}`,
  attachments: [],
  files: [],
  status: "completed",
  parts: texts.map((text, offset) => ({
    offset,
    kind: "text",
    text,
    toolId: "",
    noticeId: "",
  })),
  ...change,
})

const page = (
  messages: readonly ConversationMessage[],
  more: boolean,
): ConversationEarlier => ({
  conversationId,
  messages: [...messages],
  tools: [],
  handoffs: [],
  more,
  truncated: false,
})

const said = (messages: readonly ConversationMessage[]) =>
  messages.map((message) => [
    message.executionId,
    message.userText,
    message.parts.map((part) => part.text),
  ])

describe("earlier turns", () => {
  it("joins one turn's pieces at their seam, a part cut partway one part again", () => {
    const older = turn("b", ["first", "sec"])
    const newer = turn("b", ["ond", "third"], {
      userText: "",
      status: "running",
      startsAt: { part: 1, byte: 3 },
    })
    const [whole] = joined([turn("a", ["x"]), older], [newer]).slice(1)
    expect(whole).toMatchObject({ userText: "asked b", status: "running" })
    expect(whole?.startsAt).toBeUndefined()
    expect(whole?.parts.map((part) => part.text)).toEqual(["first", "second", "third"])
  })

  it("joins at a part's edge without merging parts", () => {
    const joinedTurns = joined(
      [turn("b", ["first"])],
      [turn("b", ["second"], { userText: "", startsAt: { part: 1, byte: 0 } })],
    )
    expect(said(joinedTurns)).toEqual([["b", "asked b", ["first", "second"]]])
  })

  it("draws a view's turns after those read before it, and its fork only once the start was read", () => {
    const fork = {
      handoffId: "f",
      kind: "fork" as const,
      atMs: 1,
      turns: [{ said: "copied" }],
    }
    const shown = view(conversationId, {
      earlier: true,
      messages: [turn("c", ["three"])],
      handoffs: [{ ...fork, turns: [] }],
    })
    // Nothing read yet: the fork waits for the earliest page.
    expect(withEarlier(shown, undefined).handoffs).toEqual([])
    const first = withPage(
      undefined,
      page([turn("b", ["two"])], true),
      startOf(shown.messages[0]!),
    )
    const some = withEarlier(shown, first)
    expect(said(some.messages)).toEqual([
      ["b", "asked b", ["two"]],
      ["c", "asked c", ["three"]],
    ])
    expect(some.earlier).toBe(true)
    expect(some.handoffs).toEqual([])
    const all = withPage(
      first,
      { ...page([turn("a", ["one"])], false), handoffs: [fork] },
      startOf(shown.messages[0]!),
    )
    const whole = withEarlier(shown, all)
    expect(whole.messages.map((message) => message.executionId)).toEqual(["a", "b", "c"])
    expect(whole.earlier).toBe(false)
    expect(whole.handoffs).toEqual([fork])
    // A view that lists everything is itself.
    const complete = view(conversationId, { messages: [turn("a", ["one"])] })
    expect(withEarlier(complete, all)).toBe(complete)
  })

  it("reads the gap once the view moved on, and joins it in", () => {
    const before = view(conversationId, {
      earlier: true,
      messages: [turn("c", ["three"])],
    })
    const held = withPage(
      undefined,
      page([turn("b", ["two"])], true),
      startOf(before.messages[0]!),
    )
    const moved = view(conversationId, { earlier: true, messages: [turn("e", ["five"])] })
    const gap = gapOf(moved, held)
    expect(gap).toEqual({
      held,
      after: { executionId: "c" },
      before: { executionId: "e" },
    })
    const filled = withGap(
      gap!.held,
      [page([turn("c", ["three"]), turn("d", ["four"])], false)],
      gap!.before,
    )
    expect(gapOf(moved, filled)).toBeUndefined()
    expect(
      withEarlier(moved, filled).messages.map((message) => message.executionId),
    ).toEqual(["b", "c", "d", "e"])
  })

  it("cuts what was read back to the turn the view now starts inside", () => {
    const long = view(conversationId, {
      earlier: true,
      messages: [turn("b", ["tail"], { userText: "", startsAt: { part: 2, byte: 0 } })],
    })
    const held = withPage(
      undefined,
      page([turn("a", ["one"]), turn("b", ["head", "middle"])], false),
      startOf(long.messages[0]!),
    )
    // The view now starts earlier in the same turn.
    const sooner = view(conversationId, {
      earlier: true,
      messages: [
        turn("b", ["middle", "tail"], { userText: "", startsAt: { part: 1, byte: 0 } }),
      ],
    })
    const gap = gapOf(sooner, held)
    expect(gap?.after).toEqual({ executionId: "b" })
    expect(gap?.before).toEqual({ executionId: "b", at: { part: 1, byte: 0 } })
    expect(said(gap!.held.messages)).toEqual([["a", "asked a", ["one"]]])
    // Until the gap is read, the turns meet without joining anything wrongly.
    expect(said(withEarlier(sooner, gap!.held).messages)).toEqual([
      ["a", "asked a", ["one"]],
      ["b", "", ["middle", "tail"]],
    ])
  })

  it("takes what a frame drew before a later point, cut by bytes as the gateway counts them", () => {
    const before = view(conversationId, {
      earlier: true,
      messages: [
        turn("a", ["one"]),
        turn("b", ["éé", "xyz"], { userText: "", startsAt: { part: 2, byte: 1 } }),
      ],
    })
    // "é" is two bytes: byte 1 of part 3 of the turn is past the first "é".
    const head = leading(before, { executionId: "b", at: { part: 2, byte: 3 } })
    expect(said(head?.messages ?? [])).toEqual([
      ["a", "asked a", ["one"]],
      ["b", "", ["é"]],
    ])
    expect(head?.messages[1]?.startsAt).toEqual({ part: 2, byte: 1 })
    expect(
      leading(before, { executionId: "b", at: { part: 1, byte: 0 } }),
    ).toBeUndefined()
    expect(leading(before, { executionId: "z" })).toBeUndefined()
    expect(leading({ ...before, truncated: true }, { executionId: "b" })).toBeUndefined()
  })

  it("reads only the stretch between when the view moves on inside the turn read up to", () => {
    const held = withPage(undefined, page([turn("a", ["one"])], false), {
      executionId: "a",
      at: { part: 1, byte: 0 },
    })
    const moved = view(conversationId, {
      earlier: true,
      messages: [turn("a", ["later"], { userText: "", startsAt: { part: 4, byte: 2 } })],
    })
    const gap = gapOf(moved, held)
    expect(gap?.held).toBe(held)
    expect(gap?.after).toEqual({ executionId: "a", at: { part: 1, byte: 0 } })
    expect(gap?.before).toEqual({ executionId: "a", at: { part: 4, byte: 2 } })
  })
})
