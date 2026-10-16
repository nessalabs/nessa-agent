import { describe, expect, it } from "vitest"
import fixtures from "../../../../protocol/product/fixtures.json"
import { eventSubscribeOptions, eventsFrame } from "./subscription-validate.js"
import { ConversationEventKind } from "../generated/product.js"

const conversationId = "00000000-0000-4000-8000-000000000001"
const base = { id: "one", cursor: "one" }

describe("semantic event contract", () => {
  it("accepts every canonical kind including sparse tool and state changes", () => {
    expect(eventsFrame(fixtures.ConversationEvents).events).toEqual(
      fixtures.ConversationEvents.events,
    )
    for (const status of [
      "queued",
      "injected",
      "settled",
      "cancelled",
      "waiting_permission",
      "permission_cancelled",
      "permission_declined",
      "permission_deadline_answered",
      "waiting_question",
      "question_closed",
      "stop_requested",
    ])
      expect(() =>
        eventsFrame({
          cursor: "one",
          events: [{ ...base, kind: "state.changed", executionId: "turn", status }],
        }),
      ).not.toThrow()
    expect(() =>
      eventsFrame({
        cursor: "one",
        events: [
          {
            ...base,
            kind: "tool.updated",
            executionId: "turn",
            toolId: "tool",
            name: "shell",
            status: "completed",
          },
        ],
      }),
    ).not.toThrow()
  })

  for (const fields of [
    { kind: "turn.ended" },
    { kind: "turn.ended", status: "settled", outcome: "completed" },
    { kind: "turn.ended", executionId: "turn", status: "settled" },
    { kind: "turn.started", executionId: "turn", status: "settled" },
    {
      kind: "turn.accepted",
      executionId: "turn",
      status: "accepted",
      outcome: "completed",
    },
    { kind: "tool.updated", toolId: "tool" },
    { kind: "tool.updated", executionId: "turn" },
    { kind: "tool.updated", executionId: "turn", toolId: "tool", outcome: "completed" },
    { kind: "state.changed", status: "opened", executionId: "turn" },
    { kind: "state.changed", status: "waiting_permission" },
    { kind: "state.changed", status: "settlement_updated", executionId: "turn" },
    { kind: "state.changed", status: "queued", executionId: "turn", outcome: "failed" },
  ])
    it(`rejects contradictory metadata ${JSON.stringify(fields)}`, () => {
      expect(() =>
        eventsFrame({ cursor: "checkpoint", events: [{ ...base, ...fields }] }),
      ).toThrow(TypeError)
    })

  it("uses the generated unique bounded subset contract for filters", () => {
    const kinds = Object.values(ConversationEventKind)
    expect(eventSubscribeOptions(conversationId, {})).toEqual({ conversationId })
    expect(eventSubscribeOptions(conversationId, { kinds })).toEqual({
      conversationId,
      kinds,
    })
    expect(
      eventSubscribeOptions(conversationId, { kinds: ["turn.ended"] }).kinds,
    ).toEqual(["turn.ended"])
    for (const invalid of [
      [],
      ["turn.ended", "turn.ended"],
      [...kinds, "turn.ended"],
    ] as const)
      expect(() => eventSubscribeOptions(conversationId, { kinds: invalid })).toThrow(
        TypeError,
      )
  })

  it("rejects explicit null, wrong types and unknown kinds rather than treating them as omission", () => {
    for (const options of [
      { kinds: null },
      { after: null },
      { replay: null },
      { kinds: "turn.ended" },
      { kinds: ["future.unknown"] },
      { after: 7 },
      { replay: "true" },
    ])
      expect(() =>
        Reflect.apply(eventSubscribeOptions, undefined, [conversationId, options]),
      ).toThrow(TypeError)
  })
})
