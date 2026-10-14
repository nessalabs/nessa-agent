/**
 * Check a question answer the way the gateway's `AcceptedAnswer` does
 * (`crates/nessa-sdk/src/domain/agent_execution/questions/value_objects/answer.rs`),
 * including the `QuestionChoice` bounds that construction applies first.
 * An answer the ask did not offer is `not-supported`. Decline is not an answer
 * and is not checked here.
 */
import { WorkspaceSourceError } from "../../application/ports"
import type { QuestionChoice, TranscriptQuestion } from "../../model/transcript"

const MAX_KEY_BYTES = 256
const MAX_OPTIONS = 32
const MAX_TEXT_BYTES = 1024
const utf8 = new TextEncoder()

const bytes = (value: string) => utf8.encode(value).length

/** The choices the ask accepts, with blank own words omitted. */
export function acceptedQuestionChoices(
  asked: TranscriptQuestion,
  choices: readonly QuestionChoice[],
): readonly QuestionChoice[] {
  const accepted: QuestionChoice[] = []
  const seenKeys = new Set<string>()
  for (const choice of choices) {
    if (choice.key.trim() === "" || bytes(choice.key) > MAX_KEY_BYTES)
      throw new WorkspaceSourceError("not-supported")
    if (choice.values.length > MAX_OPTIONS)
      throw new WorkspaceSourceError("not-supported")
    const seenValues = new Set<string>()
    for (const value of choice.values) {
      if (seenValues.has(value)) throw new WorkspaceSourceError("not-supported")
      seenValues.add(value)
    }
    const ownWords =
      choice.ownWords !== undefined && choice.ownWords.trim() !== ""
        ? choice.ownWords
        : undefined
    if (ownWords !== undefined && bytes(ownWords) > MAX_TEXT_BYTES)
      throw new WorkspaceSourceError("not-supported")
    if (seenKeys.has(choice.key)) throw new WorkspaceSourceError("not-supported")
    seenKeys.add(choice.key)
    const field = asked.questions.find((each) => each.key === choice.key)
    if (!field) throw new WorkspaceSourceError("not-supported")
    if (choice.values.length > 1 && !field.multiSelect)
      throw new WorkspaceSourceError("not-supported")
    if (
      choice.values.some(
        (value) => !field.options.some((option) => option.value === value),
      )
    )
      throw new WorkspaceSourceError("not-supported")
    if (ownWords !== undefined && !field.freeText)
      throw new WorkspaceSourceError("not-supported")
    accepted.push(
      Object.freeze({
        key: choice.key,
        values: Object.freeze([...choice.values]),
        ...(ownWords === undefined ? {} : { ownWords }),
      }),
    )
  }
  for (const field of asked.questions) {
    if (!field.required) continue
    const answered = accepted.some(
      (choice) => choice.key === field.key && choice.values.length > 0,
    )
    if (!answered) throw new WorkspaceSourceError("not-supported")
  }
  return Object.freeze(accepted)
}
