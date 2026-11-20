import { describe, expect, it } from "vitest"
import {
  ago,
  answerLabel,
  decisionLine,
  left,
  stateLabel,
  decisionsFiltered,
  decisionsInOrder,
  filterFor,
  type Decision,
  type DecisionOutcome,
} from "./decisions"

const minute = 60_000
const now = 100 * minute

function decision(askId: string, askedAt: number, outcome: DecisionOutcome): Decision {
  return {
    ask: {
      askId,
      question: `Question ${askId}?`,
      choices: [
        { label: "Ship", consequence: "Goes out today." },
        { label: "Wait", consequence: "Goes out Monday." },
      ],
      recommended: 0,
      reason: "It is ready.",
    },
    askedAtMs: askedAt * minute,
    outcome,
  }
}

const waiting = decision("w", 90, { state: "waiting" })
const decided = decision("d", 95, {
  state: "decided",
  text: "Wait",
  choice: 1,
  atMs: 96 * minute,
})
const ownWords = decision("o", 80, {
  state: "decided",
  text: "Neither",
  atMs: 81 * minute,
})
const replaced = decision("r", 85, { state: "replaced", atMs: 90 * minute })

describe("decisions", () => {
  it("list what waits first, then in the gateway's order, whatever their clocks say", () => {
    // Newest first as the gateway lists them; the oldest was asked by a clock ahead.
    const clockAhead = decision("c", 99, { state: "replaced", atMs: 99 * minute })
    expect(
      decisionsInOrder([decided, replaced, waiting, ownWords, clockAhead]).map(
        (each) => each.ask.askId,
      ),
    ).toEqual(["w", "d", "r", "o", "c"])
  })
  it("filter To decide to what waits, and Decided to everything settled, replaced too", () => {
    const all = [waiting, decided, replaced, ownWords]
    expect(decisionsFiltered(all, "waiting")).toEqual([waiting])
    expect(decisionsFiltered(all, "decided")).toEqual([decided, replaced, ownWords])
    expect(decisionsFiltered(all, "all")).toEqual(all)
  })

  it("open the dialog on To decide only for a waiting decision", () => {
    expect(filterFor(waiting)).toBe("waiting")
    expect(filterFor(decided)).toBe("all")
    expect(filterFor(undefined)).toBe("all")
  })

  it("say the answer as the choice it named, or the person's own words", () => {
    expect(answerLabel(decided)).toBe("Wait")
    expect(answerLabel(ownWords)).toBe("Your own words")
    expect(answerLabel(waiting)).toBeUndefined()
  })

  it("say in one line where each stands", () => {
    expect(decisionLine(waiting, now)).toBe("Waiting on you · asked 10m ago")
    expect(decisionLine(decided, now)).toBe("Wait · 4m ago")
    expect(decisionLine(replaced, now)).toBe("Replaced by a newer question")
  })

  it("say when auto-pick will pick, that it is paused, or that it picked", () => {
    expect(
      decisionLine({ ...waiting, autoPick: { state: "due", atMs: now + 65_000 } }, now),
    ).toBe("Waiting on you · auto-picks in 1:05")
    expect(decisionLine({ ...waiting, autoPick: { state: "paused" } }, now)).toBe(
      "Waiting on you · auto-pick paused",
    )
    const picked = {
      ...decided,
      outcome: {
        state: "decided",
        text: "Wait.",
        choice: 1,
        auto: true,
        atMs: now - minute,
      },
    } as const
    expect(decisionLine(picked, now)).toBe("Auto-picked Wait · 1m ago")
    expect(stateLabel(picked.outcome)).toBe("Auto-picked")
    expect(stateLabel(decided.outcome)).toBe("Decided")
  })

  it("count down in minutes and seconds, then hours, and stop at zero", () => {
    expect(left(now + 4 * minute + 5_000, now)).toBe("4:05")
    expect(left(now + 59 * minute + 59_000, now)).toBe("59:59")
    expect(left(now + 125 * minute, now)).toBe("2h 5m")
    expect(left(now + 1, now)).toBe("0:01")
    expect(left(now - minute, now)).toBe("0:00")
  })

  it("say how long ago in the units a list uses", () => {
    expect(ago(now, now)).toBe("just now")
    expect(ago(now - 3 * 60 * minute, now)).toBe("3h ago")
    expect(ago(now - 49 * 60 * minute, now)).toBe("2d ago")
    // A clock that runs behind never says a time to come.
    expect(ago(now + minute, now)).toBe("just now")
  })
})
