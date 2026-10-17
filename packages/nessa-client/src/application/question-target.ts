import type {
  ConversationQuestion,
  ConversationQuestionChoice,
} from "../generated/product.js"

declare const questionTarget: unique symbol

type Asked = ConversationQuestion["questions"][number]
type CapturedQuestion = Readonly<Omit<Asked, "options">> & {
  readonly options: readonly Readonly<Asked["options"][number]>[]
}

/**
 * Immutable intent captured by {@link questionTargets} from one published ask.
 * This process-local value grants no authorization. Read again after an
 * uncertain acknowledgement; copied or restored targets must be recaptured.
 */
export type QuestionTarget = Readonly<Omit<ConversationQuestion, "questions">> & {
  readonly conversationId: string
  readonly questions: readonly CapturedQuestion[]
  readonly [questionTarget]: true
}

type QuestionView = {
  readonly conversationId: string
  readonly questions: readonly ConversationQuestion[]
}
const issued = new WeakSet<QuestionTarget>()

function primitiveText(value: unknown): asserts value is string {
  if (typeof value !== "string")
    throw new TypeError("Captured question text must be a primitive string")
}
function optionalPrimitiveText(value: unknown): void {
  if (value !== undefined && value !== null) primitiveText(value)
}

/** Capture a validated view or trusted projection; this does not authenticate its source. */
export function questionTargets(view: QuestionView): readonly QuestionTarget[] {
  const conversationId = view.conversationId
  primitiveText(conversationId)
  const asks = view.questions
  const targets: QuestionTarget[] = []
  const askCount = asks.length
  for (let askIndex = 0; askIndex < askCount; askIndex += 1) {
    const sourceAsk = asks[askIndex]
    if (sourceAsk === undefined) throw new TypeError("A captured ask must be present")
    const { question, executionId, questionId, message, questions: fields } = sourceAsk
    primitiveText(question)
    primitiveText(executionId)
    primitiveText(questionId)
    primitiveText(message)
    const questions: CapturedQuestion[] = []
    const fieldCount = fields.length
    for (let fieldIndex = 0; fieldIndex < fieldCount; fieldIndex += 1) {
      const sourceField = fields[fieldIndex]
      if (sourceField === undefined)
        throw new TypeError("A captured question field must be present")
      const {
        key,
        prompt,
        header,
        multiSelect,
        freeText,
        required,
        options: sourceOptions,
      } = sourceField
      primitiveText(key)
      primitiveText(prompt)
      optionalPrimitiveText(header)
      if (
        typeof multiSelect !== "boolean" ||
        typeof freeText !== "boolean" ||
        typeof required !== "boolean"
      )
        throw new TypeError("Captured question flags must be primitive booleans")
      const options: Readonly<Asked["options"][number]>[] = []
      const optionCount = sourceOptions.length
      for (let optionIndex = 0; optionIndex < optionCount; optionIndex += 1) {
        const option = sourceOptions[optionIndex]
        if (option === undefined)
          throw new TypeError("A captured question option must be present")
        const { value, label, description } = option
        primitiveText(value)
        primitiveText(label)
        optionalPrimitiveText(description)
        options.push(Object.freeze({ value, label, description }))
      }
      questions.push(
        Object.freeze({
          key,
          prompt,
          header,
          multiSelect,
          freeText,
          required,
          options: Object.freeze(options),
        }),
      )
    }
    const target = Object.freeze({
      conversationId,
      question,
      executionId,
      questionId,
      message,
      questions: Object.freeze(questions),
    } satisfies Omit<QuestionTarget, typeof questionTarget>) as QuestionTarget
    issued.add(target)
    targets.push(target)
  }
  return Object.freeze(targets)
}

/** Read an issued target's enclosing conversation before routing local controls. */
export function questionTargetConversation(target: QuestionTarget): string {
  return questionFields(target).conversationId
}

/** @internal Serialize issued identities and correlate answers with the captured published ask. */
export function questionFields(
  target: QuestionTarget,
  choices?:
    readonly { readonly key: string; readonly values: readonly string[] }[] | null,
) {
  if (!issued.has(target))
    throw new TypeError("Use a question target captured from a view")
  for (const choice of choices ?? []) {
    const question = target.questions.find((question) => question.key === choice.key)
    if (
      !question ||
      choice.values.some(
        (value) => !question.options.some((option) => option.value === value),
      )
    )
      throw new TypeError("Select a field and choice offered by the captured question")
  }
  const { conversationId, question, executionId, questionId } = target
  return { conversationId, question, executionId, questionId }
}

/** Published question choice data with read-only option values. */
export type QuestionChoice = Readonly<Omit<ConversationQuestionChoice, "values">> & {
  readonly values: readonly string[]
}

/**
 * Copy and freeze a deliberate question reply before holding it across an await.
 * This captures indexed data, not caller-overridable array protocols (QT12).
 * It does not decide provider answer semantics; questionFields owns correlation.
 */
export function captureQuestionChoices(
  choices: readonly QuestionChoice[] | null,
): readonly QuestionChoice[] | null {
  if (choices === null) return null
  const captured: QuestionChoice[] = []
  const choiceCount = choices.length
  for (let index = 0; index < choiceCount; index += 1) {
    const sourceChoice = choices[index]
    if (sourceChoice === undefined)
      throw new TypeError("A captured question choice must be present")
    const { key, ownWords, values: sourceValues } = sourceChoice
    primitiveText(key)
    optionalPrimitiveText(ownWords)
    const values: string[] = []
    const valueCount = sourceValues.length
    for (let valueIndex = 0; valueIndex < valueCount; valueIndex += 1) {
      const value = sourceValues[valueIndex]
      if (value === undefined)
        throw new TypeError("A captured question value must be present")
      primitiveText(value)
      values.push(value)
    }
    captured.push(Object.freeze({ key, ownWords, values: Object.freeze(values) }))
  }
  return Object.freeze(captured)
}
