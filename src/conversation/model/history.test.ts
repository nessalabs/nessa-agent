import { describe, expect, it } from "vitest"
import { historyExtent, historyNotices } from "./history"
import type { ConversationTranscriptState } from "./types"

const whole: { readonly omitted: false; readonly unconfirmed: false } | undefined =
  undefined

describe("what a bounded view says about history it did not draw", () => {
  const cases: readonly (readonly [
    ConversationTranscriptState,
    boolean,
    { readonly omitted: boolean; readonly unconfirmed: boolean } | undefined,
  ])[] = [
    ["not_loaded", false, { omitted: false, unconfirmed: true }],
    ["not_loaded", true, { omitted: true, unconfirmed: true }],
    ["partial", false, { omitted: false, unconfirmed: true }],
    ["partial", true, { omitted: true, unconfirmed: true }],
    ["stale", false, { omitted: false, unconfirmed: true }],
    ["stale", true, { omitted: true, unconfirmed: true }],
    ["unknown", false, { omitted: false, unconfirmed: true }],
    ["unknown", true, { omitted: true, unconfirmed: true }],
    ["complete_empty", false, whole],
    ["complete_empty", true, { omitted: true, unconfirmed: false }],
    ["complete", false, whole],
    ["complete", true, { omitted: true, unconfirmed: false }],
  ]

  it("is omitted when the view left turns out, and unconfirmed until the gateway says the transcript is complete", () => {
    for (const [transcriptState, truncated, expected] of cases) {
      expect(historyExtent({ transcriptState, truncated })).toEqual(expected)
    }
    expect(historyExtent(undefined)).toBeUndefined()
  })

  it("names the omitted history before the unconfirmed history", () => {
    expect(historyNotices(undefined)).toEqual([])
    expect(historyNotices({ omitted: true, unconfirmed: false })).toEqual([
      { kind: "omitted", text: "Some conversation content is not shown." },
    ])
    expect(historyNotices({ omitted: false, unconfirmed: true })).toEqual([
      {
        kind: "unconfirmed",
        text: "Conversation history is not confirmed. Recent activity may be missing.",
      },
    ])
    expect(
      historyNotices({ omitted: true, unconfirmed: true }).map((notice) => notice.kind),
    ).toEqual(["omitted", "unconfirmed"])
  })

  it("says earlier turns are not shown only where they are not read as the person scrolls back", () => {
    const history = historyExtent({
      transcriptState: "complete",
      truncated: false,
      earlier: true,
    })
    expect(history).toEqual({ omitted: false, unconfirmed: false, earlier: true })
    expect(historyNotices(history, { pagesEarlier: true })).toEqual([])
    expect(historyNotices(history).map((notice) => notice.kind)).toEqual(["omitted"])
  })
})
