import { expect, it, vi } from "vitest"
import { questionTargets, type QuestionTarget } from "../index.js"
import {
  createConversationApi,
  type ConversationApi,
} from "../presentation/conversation-api.js"
const first = "00000000-0000-4000-8000-000000000001"
const second = "00000000-0000-4000-8000-000000000002"
function source(conversationId = first) {
  return {
    conversationId,
    questions: ["a", "b"].map((suffix) => ({
      executionId: `turn-${suffix}`,
      questionId: `ask-${suffix}`,
      message: `Ask ${suffix}`,
      questions: [
        {
          key: `field-${suffix}`,
          prompt: "Choose",
          required: true,
          multiSelect: false,
          freeText: false,
          options: [{ value: `value-${suffix}`, label: `Label ${suffix}` }],
        },
      ],
    })),
  }
}
function requester() {
  const request = vi.fn(async (_method: string, params: unknown) => ({
    requestId: (params as { requestId: string }).requestId,
    applied: true,
  }))
  return { request, api: createConversationApi({ request }, () => "action") }
}
it("QT1/QT5: answers and declines retain each original conversation, turn and ask", async () => {
  const [a, b] = questionTargets(source())
  const [other] = questionTargets(source(second))
  const { api, request } = requester()
  await api.answerQuestion(a!, [{ key: "field-a", values: ["value-a"] }])
  await api.answerQuestion(b!, null)
  await api.answerQuestion(other!, null)
  expect(request.mock.calls.map((call) => call[1])).toEqual([
    {
      conversationId: first,
      executionId: "turn-a",
      questionId: "ask-a",
      requestId: "action",
      choices: [{ key: "field-a", values: ["value-a"] }],
    },
    {
      conversationId: first,
      executionId: "turn-b",
      questionId: "ask-b",
      requestId: "action",
      choices: null,
    },
    {
      conversationId: second,
      executionId: "turn-a",
      questionId: "ask-a",
      requestId: "action",
      choices: null,
    },
  ])
})
it("QT2: source mutation cannot alter captured identity, rendering or offered choices", async () => {
  const view = source()
  const [target] = questionTargets(view)
  view.conversationId = second
  Object.assign(view.questions[0]!, {
    executionId: "turn-b",
    questionId: "ask-b",
    message: "Replacement",
  })
  view.questions[0]!.questions[0]!.key = "field-b"
  Object.assign(view.questions[0]!.questions[0]!.options[0]!, {
    value: "value-b",
    label: "Replacement",
  })
  expect(target!.message).toBe("Ask a")
  expect(target!.questions[0]!.options[0]!.label).toBe("Label a")
  for (const object of [
    target!,
    target!.questions,
    target!.questions[0]!,
    target!.questions[0]!.options,
    target!.questions[0]!.options[0]!,
  ])
    expect(Object.isFrozen(object)).toBe(true)
  expect(() => Object.assign(target!, { conversationId: second })).toThrow(TypeError)
  expect(() =>
    Object.assign(target!.questions[0]!.options[0]!, { value: "value-b" }),
  ).toThrow(TypeError)
  const { api, request } = requester()
  await api.answerQuestion(target!, [{ key: "field-a", values: ["value-a"] }])
  expect(request.mock.calls[0]![1]).toMatchObject({
    conversationId: first,
    executionId: "turn-a",
    questionId: "ask-a",
  })
})
it("QT3: copied, forged and mixed valid identities fail before RPC", () => {
  const [target] = questionTargets(source())
  const { api, request } = requester()
  for (const forged of [
    { ...target },
    { ...target, conversationId: second },
    { ...target, executionId: "turn-b" },
    { ...target, questionId: "ask-b" },
    JSON.parse(JSON.stringify(target)) as QuestionTarget,
  ])
    expect(() => api.answerQuestion(forged, null)).toThrow(TypeError)
  expect(request).not.toHaveBeenCalled()
})
it("QT4: another ask's field or option fails before RPC", () => {
  const [target] = questionTargets(source())
  const { api, request } = requester()
  for (const choice of [
    { key: "field-b", values: ["value-b"] },
    { key: "field-b", values: ["value-a"] },
    { key: "field-a", values: ["value-b"] },
  ])
    expect(() => api.answerQuestion(target!, [choice])).toThrow(TypeError)
  expect(request).not.toHaveBeenCalled()
})
it("QT5: semantic answer rules stay with the SDK rather than the capture primitive", async () => {
  const [target] = questionTargets(source())
  const { api, request } = requester()
  await api.answerQuestion(target!, [])
  await api.answerQuestion(target!, [
    { key: "field-a", values: ["value-a", "value-a"], ownWords: "SDK decides" },
  ])
  expect(request).toHaveBeenCalledTimes(2)
})
function rejectedSignatures(api: ConversationApi, target: QuestionTarget) {
  // @ts-expect-error a private brand excludes arbitrary identity tuples
  const forged: QuestionTarget = {
    conversationId: first,
    executionId: "turn-a",
    questionId: "ask-a",
    message: "Ask",
    questions: [],
  }
  // @ts-expect-error captured identities are immutable
  target.questionId = "ask-b"
  // @ts-expect-error published options are immutable
  target.questions[0]!.options[0]!.value = "value-b"
  // @ts-expect-error separate identity arguments are no longer accepted
  void api.answerQuestion(first, "turn-a", "ask-a", null)
  return forged
}
void rejectedSignatures
