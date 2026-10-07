import { describe, expect, it } from "vitest"
import { retryBudgetSession } from "../../../workspace/adapters/in-memory/sample-labs"
import { sampleSubagentSource, type SampleSubagentSchedule } from "./sample-source"

function clock(
  nowAt = 1_000_000,
): SampleSubagentSchedule & { readonly pending: Array<() => void> } {
  const pending: Array<() => void> = []
  return {
    pending,
    now: () => nowAt,
    after(_ms, run) {
      pending.push(run)
      return () => {
        const at = pending.indexOf(run)
        if (at >= 0) pending.splice(at, 1)
      }
    },
  }
}

function messagesOf(source: ReturnType<typeof sampleSubagentSource>): number {
  const read = source.forSession(retryBudgetSession)
  if (read.kind !== "ready") throw new Error(read.kind)
  const mara = read.subagents.find((each) => each.id === "mara")
  if (!mara) throw new Error("no mara")
  return mara.conversation.messages.length
}

describe("the sample subagent source", () => {
  it("notifies when a subagent's conversation grows", () => {
    const time = clock()
    const source = sampleSubagentSource(time)
    const heard: number[] = []
    source.subscribe(() => heard.push(messagesOf(source)))
    const before = messagesOf(source)
    expect(time.pending).toHaveLength(3)
    expect(source.forSession(retryBudgetSession)).toBe(
      source.forSession(retryBudgetSession),
    )
    time.pending[0]()
    expect(heard).toEqual([before + 1])
    expect(messagesOf(source)).toBe(before + 1)
    time.pending[0]()
    expect(messagesOf(source)).toBe(before + 2)
    source.dispose()
  })

  it("has nothing for a conversation it does not fill", () => {
    const source = sampleSubagentSource(clock())
    expect(source.forSession("other")).toEqual({
      kind: "ready",
      subagents: [],
      unreadable: [],
    })
    source.dispose()
  })
})
