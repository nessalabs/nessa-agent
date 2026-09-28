/**
 * A peek at a session without opening it: what its agent is doing now, the
 * last few things it did this turn, and the last thing it said. Read from
 * the session's conversation; nothing here is kept apart from it.
 */
import {
  messageText,
  type Activity,
  type StepPart,
  type Transcript,
} from "../../../workspace/model/transcript"

/** How many of the turn's steps a peek shows; the rest are counted. */
export const peekSteps = 5

export interface Peek {
  readonly activity: Activity | null
  /** The turn's last steps, in the order taken. */
  readonly steps: readonly StepPart[]
  /** How many of the turn's steps came before those. */
  readonly earlier: number
  /** The agent's last words, as plain text, and when. */
  readonly said: { readonly text: string; readonly at: number } | null
}

/**
 * The peek at a conversation. The turn is everything the agent said since
 * the person last wrote; before the person has written, the whole
 * conversation.
 */
export function peekOf(transcript: Transcript): Peek {
  const messages = transcript.messages
  let start = 0
  for (let index = messages.length - 1; index >= 0; index--)
    if (messages[index].role === "user") {
      start = index + 1
      break
    }
  const turn = messages.slice(start).filter((message) => message.role === "agent")
  const steps = turn.flatMap((message) =>
    message.parts.filter((part): part is StepPart => part.kind === "step"),
  )
  const lastWords = [...messages]
    .reverse()
    .find((message) => message.role === "agent" && messageText(message).trim() !== "")
  return {
    activity: transcript.activity,
    steps: steps.slice(-peekSteps),
    earlier: Math.max(0, steps.length - peekSteps),
    said: lastWords ? { text: messageText(lastWords).trim(), at: lastWords.at } : null,
  }
}
