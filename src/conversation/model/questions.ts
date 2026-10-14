/**
 * Whether an ask can be sent, and the choices that send carries.
 *
 * The agent's question is the owner of what an answer must contain: a
 * required field needs one of its options, and words of the answerer's own
 * do not stand in for that option. Skipping the whole ask is a separate
 * answer (`choices` null) and is always possible. Both surfaces that draw
 * the ask use this, so one of them cannot accept an answer the other holds
 * back.
 */

export interface QuestionField {
  readonly key: string
  readonly required: boolean
}

/** True when every required field has an option and at least one field was answered. */
export function questionAnswerReady(
  questions: readonly QuestionField[],
  chosen: ReadonlyMap<string, readonly string[]>,
  ownWords: ReadonlyMap<string, string>,
): boolean {
  const answered = (question: QuestionField) =>
    Boolean(chosen.get(question.key)?.length || ownWords.get(question.key)?.trim())
  return (
    questions.some(answered) &&
    questions.every(
      (question) => !question.required || Boolean(chosen.get(question.key)?.length),
    )
  )
}

/** The fields somebody touched, omitting one left blank. */
export function questionChoices<Field extends { readonly key: string }>(
  questions: readonly Field[],
  chosen: ReadonlyMap<string, readonly string[]>,
  ownWords: ReadonlyMap<string, string>,
): { key: string; values: string[]; ownWords?: string }[] {
  return questions.flatMap((question) => {
    const values = [...(chosen.get(question.key) ?? [])]
    const words = ownWords.get(question.key)?.trim()
    if (values.length === 0 && !words) return []
    return [{ key: question.key, values, ...(words ? { ownWords: words } : {}) }]
  })
}
