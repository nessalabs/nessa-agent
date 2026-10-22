import { describe, expect, it } from "vitest"
import {
  conversationPermission,
  conversationQuestion,
  conversationView,
} from "../../packages/nessa-client/src/protocol/conversation-validate"
import {
  committedQuestion,
  committedReview,
  decodePanelView,
  publishedQuestionView,
  publishedView,
  questionView,
  states,
  view,
} from "./fixture-state"

describe("committed browser fixture inputs", () => {
  it.each(states)("keeps %s authority bounded by the live complete view", (state) => {
    for (const live of [false, true]) {
      const shown = view(state, live)
      expect(shown.permissions).toEqual(
        state === "complete" && live ? [committedReview] : [],
      )
      expect(publishedView(state, live).transcriptState).toBe(state)
    }
  })

  it("loads keyed questions without putting their detail in the listed view", () => {
    for (const limited of [false, true]) {
      expect(publishedQuestionView(limited).questions).toEqual([
        { question: committedQuestion.question, message: committedQuestion.message },
      ])
      const shown = questionView(limited)
      expect(shown.questions).toEqual([committedQuestion])
      expect(shown.questions[0]!.questions.map((one) => one.key)).toEqual([
        "environment_0",
        "environment_1",
        "environment_2",
      ])
      expect(shown.truncated).toBe(limited)
    }
  })

  it("does not give an unknown listed key the fixture's loaded authority", () => {
    const published = publishedQuestionView(false)
    published.questions = [{ ...published.questions[0]!, question: "a".repeat(32) }]
    const shown = decodePanelView(published, published.conversationId)
    expect(shown.questions).toEqual([])
    expect(shown.questionsWaiting).toHaveLength(1)
  })

  it("refuses the original missing permission key and inline question shapes", () => {
    const { review: _review, ...withoutKey } = committedReview
    expect(() => conversationPermission(withoutKey)).toThrow()
    const { question: _question, ...inline } = committedQuestion
    expect(() => conversationQuestion(inline)).toThrow()
    const published = { ...publishedQuestionView(false), questions: [inline] }
    expect(() => conversationView(published, published.conversationId)).toThrow()
  })
})
