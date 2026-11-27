import { expect, it } from "vitest"
import { summary } from "../testing"
import {
  acceptedByDefault,
  answerText,
  decisionsByAgent,
  optionFacts,
  waitingDecisions,
} from "./waiting-decisions"
import type { OwnerAsk } from "./workspace-index"

const ask = (askId: string, extra: Partial<OwnerAsk> = {}): OwnerAsk => ({
  askId,
  question: `Question ${askId}?`,
  choices: [
    { label: "Ship", consequence: "It goes out.", undo: "easy" },
    { label: "Wait", consequence: "It waits.", undo: "costly" },
  ],
  recommended: 0,
  reason: "Nothing else waits on it.",
  ...extra,
})

it("queues every asked chat, longest waiting first, and leaves out access requests", () => {
  const decisions = waitingDecisions([
    summary("late", "c", 300, "needs-you", { ask: ask("a") }),
    summary("none", "c", 100),
    summary("early", "c", 200, "needs-you", { ask: ask("b"), now: "  Migrating  " }),
    summary("access", "c", 50, "needs-you", {
      ask: ask("c", { access: { mode: "full", grants: [] } }),
    }),
  ])
  expect(decisions.map((each) => each.sessionId)).toEqual(["early", "late"])
  expect(decisions[0]).toMatchObject({ doing: "Migrating", askedAtMs: 200 })
  expect(decisions[1]).not.toHaveProperty("doing")
})

it("groups the queue under each agent in the order they first wait", () => {
  const decisions = waitingDecisions([
    summary("a", "c", 1, "needs-you", { ask: ask("a") }),
    summary("b", "c", 2, "needs-you", {
      ask: ask("b"),
      model: { provider: "openai", modelId: "gpt-5" },
    }),
    summary("c", "c", 3, "needs-you", { ask: ask("c") }),
  ])
  const groups = decisionsByAgent(decisions)
  expect(groups.map((group) => group.decisions.map((each) => each.sessionId))).toEqual([
    ["a", "c"],
    ["b"],
  ])
})

it("sends a choice's label, with a note after it, or the person's own words", () => {
  const held = ask("a")
  expect(answerText(held, { choice: 1 })).toBe("Wait")
  expect(answerText(held, { choice: 0, note: "  after lunch " })).toBe(
    "Ship. after lunch",
  )
  expect(answerText(held, { own: "  Neither, ask Dana " })).toBe("Neither, ask Dana")
  expect(answerText(held, { own: "   " })).toBeUndefined()
  expect(answerText(held, { choice: 5 })).toBeUndefined()
})

it("ticks a suggestion for Accept all only when it is easy to undo", () => {
  const decide = (recommended: number, undo?: "easy" | "costly" | "no") =>
    acceptedByDefault(
      waitingDecisions([
        summary("a", "c", 1, "needs-you", {
          ask: ask("a", {
            recommended,
            choices: [
              { label: "One", consequence: "", ...(undo ? { undo } : {}) },
              { label: "Two", consequence: "", undo: "easy" },
            ],
          }),
        }),
      ])[0]!,
    )
  expect(decide(0, "easy")).toBe(true)
  expect(decide(0, "costly")).toBe(false)
  expect(decide(0, "no")).toBe(false)
  expect(decide(0)).toBe(false)
})

it("compares options only on the facts the agent gave, and undo only when it differs", () => {
  expect(optionFacts(ask("a"))).toEqual(["consequence", "undo"])
  expect(
    optionFacts(
      ask("b", {
        choices: [
          {
            label: "One",
            consequence: "",
            undo: "easy",
            pros: ["Fast"],
            effort: "small",
          },
          { label: "Two", consequence: "", undo: "easy", cons: ["Slow"] },
        ],
      }),
    ),
  ).toEqual(["consequence", "pros", "cons", "effort"])
})
