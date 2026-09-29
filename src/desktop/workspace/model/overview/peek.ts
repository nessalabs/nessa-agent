/**
 * A peek at a session without opening it tells its turn's story, top to
 * bottom: what the person last asked, then everything the agent did and said
 * since, in the order it happened — its steps, its edits, its words — and
 * what it is doing now. Read from the session's conversation; nothing here
 * is kept apart from it, and nothing is summarised: what is going on, in a
 * line, is the source's to say (`SessionSummary.now`).
 */
import type { Activity, Message, Transcript } from "../transcript"

export interface Peek {
  /** The person's latest message; `null` before they have written. */
  readonly asked: Message | null
  /** The agent's messages since, in order — every one, before the person has written. */
  readonly since: readonly Message[]
  readonly activity: Activity | null
}

/** The story of a conversation's current turn. */
export function peekOf(transcript: Transcript): Peek {
  const messages = transcript.messages
  let last = messages.length - 1
  while (last >= 0 && messages[last].role !== "user") last--
  return {
    asked: last === -1 ? null : messages[last],
    since: messages.slice(last + 1),
    activity: transcript.activity,
  }
}
