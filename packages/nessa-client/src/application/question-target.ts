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

/** Capture a validated view or trusted projection; this does not authenticate its source. */
export function questionTargets(view: QuestionView): readonly QuestionTarget[] {
  return Object.freeze(
    view.questions.map((ask) => {
      const target = Object.freeze({
        conversationId: view.conversationId,
        executionId: ask.executionId,
        questionId: ask.questionId,
        message: ask.message,
        questions: Object.freeze(
          ask.questions.map((question) =>
            Object.freeze({
              ...question,
              options: Object.freeze(
                question.options.map((option) => Object.freeze({ ...option })),
              ),
            }),
          ),
        ),
      } satisfies Omit<QuestionTarget, typeof questionTarget>) as QuestionTarget
      issued.add(target)
      return target
    }),
  )
}

/** Read an issued target's enclosing conversation before routing local controls. */
export function questionTargetConversation(target: QuestionTarget): string {
  return questionFields(target).conversationId
}

/** @internal Serialize issued identities and correlate answers with the captured published ask. */
export function questionFields(
  target: QuestionTarget,
  choices?: readonly ConversationQuestionChoice[] | null,
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
  const { conversationId, executionId, questionId } = target
  return { conversationId, executionId, questionId }
}
