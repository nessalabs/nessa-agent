/**
 * One saved record this read could not apply, as one line in the transcript.
 * The sentence is the same on every surface that shows a conversation.
 *
 * `data-unreadable` is the record's id. A future report action attaches to
 * this line; `data-unreadable-report` is the place it mounts. This line has
 * no report control.
 */
export const UNREADABLE_SENTENCE = "Couldn't read this part of the conversation"

export function UnreadablePartLine({ part }: { part: { id: string } }) {
  return (
    <p
      data-unreadable={part.id}
      data-unreadable-report=""
      className="m-0 min-w-0 font-sans nessa-text-1 text-muted-foreground [overflow-wrap:anywhere]"
    >
      {UNREADABLE_SENTENCE}
    </p>
  )
}

/** The unreadable records placed at one point of a transcript, in order; nothing for none. */
export function UnreadableParts({ parts }: { parts: readonly { id: string }[] }) {
  return parts.map((part) => <UnreadablePartLine key={part.id} part={part} />)
}
