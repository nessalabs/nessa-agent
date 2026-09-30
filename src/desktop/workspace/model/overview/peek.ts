/**
 * A peek at a session without opening it tells its turn's story, top to
 * bottom: what the person last asked, then what the agent did and said
 * since, in the order it happened — its steps, its edits, its words — and
 * what it is doing now. Read from the session's conversation; nothing here
 * is kept apart from it, and nothing is summarised: what is going on, in a
 * line, is the source's to say (`SessionSummary.now`).
 *
 * A peek is a glance, drawn afresh for each session the keyboard lands on,
 * so it draws a bounded part of the turn: its latest `peekParts` parts —
 * steps, paragraphs, code, lists — and says there is more above, which the
 * session shows in full. A widget is not one of them: a plugin's view is the
 * session's to draw, so it is dropped before the parts are counted.
 */
import type { Activity, Message, Transcript } from "../transcript"

/** The most of a turn's parts a peek draws: its latest. */
export const peekParts = 24

export interface Peek {
  /** The person's latest message; `null` before they have written. */
  readonly asked: Message | null
  /**
   * The agent's messages since, in order, holding the latest `peekParts`
   * parts between them — the conversation's own messages, but for the
   * oldest drawn, which is cut to its latest parts when the turn holds more.
   */
  readonly since: readonly Message[]
  /** Whether the turn holds more than is drawn, earlier than `since`. */
  readonly earlier: boolean
  readonly activity: Activity | null
}

/** The story of a conversation's current turn, its latest `limit` parts drawn. */
export function peekOf(transcript: Transcript, limit = peekParts): Peek {
  const messages = transcript.messages
  let last = messages.length - 1
  while (last >= 0 && messages[last].role !== "user") last--
  const since: Message[] = []
  let room = limit
  let total = 0
  for (let index = messages.length - 1; index > last; index--) {
    const message = withoutWidgets(messages[index])
    total += message.parts.length
    if (room === 0) continue
    const drawn =
      message.parts.length <= room
        ? message
        : { ...message, parts: message.parts.slice(-room) }
    since.unshift(drawn)
    room -= drawn.parts.length
  }
  return {
    asked: last === -1 ? null : messages[last],
    since,
    earlier: total > limit,
    activity: transcript.activity,
  }
}

/** `message` without its widget parts; the same message when it has none. */
function withoutWidgets(message: Message): Message {
  return message.parts.some((part) => part.kind === "widget")
    ? { ...message, parts: message.parts.filter((part) => part.kind !== "widget") }
    : message
}
