import { expect, it, vi } from "vitest"
import {
  captureQuestionChoices,
  questionTargets,
  type QuestionTarget,
  type QuestionChoice,
} from "../index.js"
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
      question: suffix.repeat(32),
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
      question: "a".repeat(32),
      questionId: "ask-a",
      requestId: "action",
      choices: [{ key: "field-a", values: ["value-a"] }],
    },
    {
      conversationId: first,
      executionId: "turn-b",
      question: "b".repeat(32),
      questionId: "ask-b",
      requestId: "action",
      choices: null,
    },
    {
      conversationId: second,
      executionId: "turn-a",
      question: "a".repeat(32),
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

it("QT11: queued serialization retains the exact answer snapshot that passed correlation", async () => {
  const [target] = questionTargets(source())
  let release!: () => void
  const gate = new Promise<void>((done) => {
    release = done
  })
  let serialized: unknown
  let captured: unknown
  const request = vi.fn(async (_method: string, params: unknown) => {
    await gate
    serialized = JSON.parse(JSON.stringify(params))
    captured = params
    return { requestId: "action", applied: true }
  })
  const api = createConversationApi({ request }, () => "action")
  const choices = [{ key: "field-a", values: ["value-a"], ownWords: "Original" }]
  const pending = api.answerQuestion(target!, choices)
  choices[0]!.values[0] = "value-b"
  choices[0]!.key = "field-b"
  choices[0]!.ownWords = "Replacement"
  choices.push({ key: "field-b", values: ["value-b"], ownWords: "Added" })
  release()
  await expect(pending).resolves.toEqual({ requestId: "action", applied: true })
  expect(request).toHaveBeenCalledTimes(1)
  expect(serialized).toEqual({
    conversationId: first,
    executionId: "turn-a",
    question: "a".repeat(32),
    questionId: "ask-a",
    requestId: "action",
    choices: [{ key: "field-a", values: ["value-a"], ownWords: "Original" }],
  })
  const fields = captured as { choices: { values: string[] }[] }
  expect(Object.isFrozen(fields.choices)).toBe(true)
  expect(Object.isFrozen(fields.choices[0])).toBe(true)
  expect(Object.isFrozen(fields.choices[0]!.values)).toBe(true)
})

it("QT12: array subclass hooks cannot keep caller-owned answer values behind capture", async () => {
  const [target] = questionTargets(source())
  const valueSource = ["value-a"]
  let reads = 0
  class Choices extends Array<{ key: string; values: string[] }> {
    override map<U>(): U[] {
      return this as unknown as U[]
    }
  }
  const choices = new Choices({
    key: "field-a",
    get values() {
      reads += 1
      return valueSource
    },
  })
  let release!: () => void
  const gate = new Promise<void>((done) => {
    release = done
  })
  let serialized: unknown
  const request = vi.fn(async (_method: string, params: unknown) => {
    await gate
    serialized = JSON.parse(JSON.stringify(params))
    return { requestId: "action", applied: true }
  })
  const api = createConversationApi({ request }, () => "action")
  const pending = api.answerQuestion(target!, choices)
  valueSource[0] = "value-b"
  release()
  await pending
  expect(reads).toBe(1)
  expect(serialized).toMatchObject({ choices: [{ key: "field-a", values: ["value-a"] }] })
})

it("QT12: overridden projection map cannot retain mutable offered options", async () => {
  const view = source()
  type Option = (typeof view.questions)[0]["questions"][0]["options"][0]
  class Options extends Array<Option> {
    override map<U>(): U[] {
      return this as unknown as U[]
    }
  }
  const option = { value: "value-a", label: "Original" }
  view.questions[0]!.questions[0]!.options = new Options(option)
  const [target] = questionTargets(view)
  Object.assign(option, { value: "value-b", label: "Changed" })
  expect(target!.questions[0]!.options[0]).toEqual({
    value: "value-a",
    label: "Original",
  })
  const { api, request } = requester()
  await api.answerQuestion(target!, [{ key: "field-a", values: ["value-a"] }])
  expect(request).toHaveBeenCalledTimes(1)
  expect(() =>
    api.answerQuestion(target!, [{ key: "field-a", values: ["value-b"] }]),
  ).toThrow(TypeError)
})

it.each(["asks", "fields"] as const)(
  "QT12: %s array hooks cannot replace factory-issued targets with source objects",
  async (level) => {
    class SourceArray<T> extends Array<T> {
      override map<U>(): U[] {
        return this as unknown as U[]
      }
    }
    const view = source()
    if (level === "asks") view.questions = new SourceArray(...view.questions)
    else view.questions[0]!.questions = new SourceArray(...view.questions[0]!.questions)
    const [target] = questionTargets(view)
    expect(target).toMatchObject({
      conversationId: first,
      executionId: "turn-a",
      questionId: "ask-a",
    })
    view.questions[0]!.questions[0]!.key = "changed"
    expect(target!.questions[0]!.key).toBe("field-a")
    const { api, request } = requester()
    await api.answerQuestion(target!, [{ key: "field-a", values: ["value-a"] }])
    expect(request).toHaveBeenCalledTimes(1)
  },
)

it("QT12: choice value iteration does not substitute values for indexed source data", async () => {
  class Values extends Array<string> {
    override [Symbol.iterator](): ArrayIterator<string> {
      return ["value-b"].values()
    }
  }
  const [target] = questionTargets(source())
  const { api, request } = requester()
  await api.answerQuestion(target!, [{ key: "field-a", values: new Values("value-a") }])
  expect(request.mock.calls[0]![1]).toMatchObject({
    choices: [{ key: "field-a", values: ["value-a"] }],
  })
})

it("QT12: reading an indexed value cannot extend the captured choice list", async () => {
  const values = ["value-a"]
  Object.defineProperty(values, 0, {
    get: () => {
      values[1] = "value-b"
      return "value-a"
    },
  })
  const [target] = questionTargets(source())
  const { api, request } = requester()
  await api.answerQuestion(target!, [{ key: "field-a", values }])
  expect(request.mock.calls[0]![1]).toMatchObject({
    choices: [{ key: "field-a", values: ["value-a"] }],
  })
})

it("QT12: sparse source arrays are refused rather than captured as incomplete intent", () => {
  const { api, request } = requester()
  const [target] = questionTargets(source())
  expect(() => api.answerQuestion(target!, new Array(1))).toThrow(TypeError)
  expect(() =>
    api.answerQuestion(target!, [{ key: "field-a", values: new Array(1) }]),
  ).toThrow(TypeError)
  for (const level of ["asks", "fields", "options"]) {
    const view = source()
    if (level === "asks") view.questions = new Array(1)
    else if (level === "fields") view.questions[0]!.questions = new Array(1)
    else view.questions[0]!.questions[0]!.options = new Array(1)
    expect(() => questionTargets(view)).toThrow(TypeError)
  }
  expect(request).not.toHaveBeenCalled()
})
it("the public choice capture retains valid replies and null declines", () => {
  expect(captureQuestionChoices(null)).toBeNull()
  const original = [{ key: "field-a", values: ["value-a"], ownWords: "Original" }]
  const captured = captureQuestionChoices(original)
  original[0]!.values[0] = "value-b"
  expect(captured).toEqual([
    { key: "field-a", values: ["value-a"], ownWords: "Original" },
  ])
  expect(Object.isFrozen(captured)).toBe(true)
  expect(Object.isFrozen(captured![0]!.values)).toBe(true)
})
function rejectedCapturedChoices() {
  const captured = captureQuestionChoices([{ key: "field-a", values: ["value-a"] }])
  if (captured) {
    // @ts-expect-error captured question choices are immutable value objects
    captured[0]!.key = "field-b"
    // @ts-expect-error captured option lists cannot be extended
    captured[0]!.values.push("value-b")
  }
}
void rejectedCapturedChoices

it("QT12: serialization hooks on source records do not replace captured choice data", async () => {
  const [target] = questionTargets(source())
  const { api, request } = requester()
  const choice = {
    key: "field-a",
    values: ["value-a"],
    toJSON: () => ({ key: "field-b", values: ["value-b"] }),
  }
  await api.answerQuestion(target!, [choice])
  expect(JSON.parse(JSON.stringify(request.mock.calls[0]![1]))).toMatchObject({
    choices: [{ key: "field-a", values: ["value-a"] }],
  })
})

it("QT12: one enclosing conversation identity is read for the whole projection", async () => {
  const view = source()
  let reads = 0
  Object.defineProperty(view, "conversationId", {
    get: () => (++reads === 1 ? first : second),
  })
  const targets = questionTargets(view)
  expect(reads).toBe(1)
  expect(targets.map((target) => target.conversationId)).toEqual([first, first])
  const { api, request } = requester()
  await api.answerQuestion(targets[0]!, null)
  await api.answerQuestion(targets[1]!, null)
  expect(request).toHaveBeenCalledTimes(2)
})

it("QT12: inherited data getters are captured without inheriting source behavior", async () => {
  class Choice {
    get key() {
      return "field-a"
    }
    get values() {
      return ["value-a"]
    }
    toJSON() {
      return { key: "field-b", values: ["value-b"] }
    }
  }
  const [target] = questionTargets(source())
  const { api, request } = requester()
  await api.answerQuestion(target!, [new Choice()])
  expect(JSON.parse(JSON.stringify(request.mock.calls[0]![1]))).toMatchObject({
    choices: [{ key: "field-a", values: ["value-a"] }],
  })
})

it("QT14: mutable own-words serialization is refused before queued RPC", async () => {
  const [target] = questionTargets(source())
  const words = {
    value: "Original",
    toJSON() {
      return this.value
    },
  }
  let release!: () => void
  const gate = new Promise<void>((done) => {
    release = done
  })
  const request = vi.fn(async () => {
    await gate
    return { requestId: "action", applied: true }
  })
  const api = createConversationApi({ request }, () => "action")
  let outcome: unknown
  try {
    const pending = api.answerQuestion(target!, [
      { key: "field-a", values: ["value-a"], ownWords: words },
    ] as unknown as QuestionChoice[])
    words.value = "Changed"
    release()
    outcome = await pending
  } catch (error) {
    release()
    outcome = error
  }
  expect(outcome).toBeInstanceOf(TypeError)
  expect(request).not.toHaveBeenCalled()
})

it("QT14: choice capture rejects boxed scalars and preserves primitive/null reply data", () => {
  const mutable = { toJSON: () => "changed" }
  for (const choice of [
    { key: mutable, values: ["value-a"] },
    { key: "field-a", values: [mutable] },
    { key: "field-a", values: [], ownWords: mutable },
  ])
    expect(() => captureQuestionChoices([choice] as unknown as QuestionChoice[])).toThrow(
      TypeError,
    )
  expect(
    captureQuestionChoices([
      { key: "unoffered", values: ["SDK checks"], ownWords: null },
      { key: "field-a", values: [], ownWords: "Primitive" },
    ]),
  ).toEqual([
    { key: "unoffered", values: ["SDK checks"], ownWords: null },
    { key: "field-a", values: [], ownWords: "Primitive" },
  ])
})

it.each([
  "conversationId",
  "question",
  "executionId",
  "questionId",
  "message",
  "key",
  "prompt",
  "header",
  "multiSelect",
  "freeText",
  "required",
  "value",
  "label",
  "description",
])(
  "QT14: target capture rejects mutable %s representation rather than retaining its reference",
  (field) => {
    const view = source()
    const mutable = { toJSON: () => "changed" }
    const ask = view.questions[0]!
    const question = ask.questions[0]!
    const option = question.options[0]!
    if (field === "conversationId") Object.assign(view, { [field]: mutable })
    else if (["question", "executionId", "questionId", "message"].includes(field))
      Object.assign(ask, { [field]: mutable })
    else if (["value", "label", "description"].includes(field))
      Object.assign(option, { [field]: mutable })
    else Object.assign(question, { [field]: mutable })
    expect(() => questionTargets(view)).toThrow(TypeError)
  },
)

it("QT14: nullable published display text retains primitive flag meanings", () => {
  const view = source()
  Object.assign(view.questions[0]!.questions[0]!, {
    header: null,
    multiSelect: false,
    freeText: true,
    required: false,
  })
  Object.assign(view.questions[0]!.questions[0]!.options[0]!, { description: null })
  const [target] = questionTargets(view)
  expect(target!.questions[0]).toMatchObject({
    header: null,
    multiSelect: false,
    freeText: true,
    required: false,
    options: [{ description: null }],
  })
})
