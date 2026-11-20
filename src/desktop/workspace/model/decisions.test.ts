import { describe, expect, it } from "vitest"
import {
  ago,
  answerLabel,
  decisionLine,
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
  it("list what waits first, then newest first", () => {
    expect(
      decisionsInOrder([ownWords, decided, replaced, waiting]).map(
        (each) => each.ask.askId,
      ),
    ).toEqual(["w", "d", "r", "o"])
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

  it("say how long ago in the units a list uses", () => {
    expect(ago(now, now)).toBe("just now")
    expect(ago(now - 3 * 60 * minute, now)).toBe("3h ago")
    expect(ago(now - 49 * 60 * minute, now)).toBe("2d ago")
    // A clock that runs behind never says a time to come.
    expect(ago(now + minute, now)).toBe("just now")
  })
})
