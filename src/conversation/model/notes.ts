import type { AgentSource } from "./authorship"

/**
 * A note left in a conversation by the owner, a share holder, or an agent.
 * A note is not a
 * turn and not the person's message, so it is shown apart from both, with
 * who left it and where it stands.
 */
export type Note = {
  /** The conversation's own name for the note. */
  id: string
  /** What it says, exactly as it was left; shown as plain text. */
  text: string
  author: NoteAuthor
  /**
   * Where it stands. `recorded` is a note left only to be read; `pending`
   * waits for the next turn a person starts; `assigned` was given to a turn;
   * `retired` will be given to none, for `retiredReason`.
   */
  deliveryState: "recorded" | "pending" | "assigned" | "retired"
  /**
   * Why no turn will take it. The view carries it exactly when the note is
   * `retired`: the client refuses one that does not (`conversation-api.test.ts`).
   */
  retiredReason?: "expired" | "permission_lost"
  /**
   * The last turn the conversation had accepted when the note was left, so it
   * is shown after that turn; absent when it was left before any turn.
   */
  afterExecutionId?: string
}

/** Who left a note, and from which surface. */
export type NoteAuthor = {
  kind: "owner" | "grant" | "agent"
  /** The surface the gateway verified it was left from, a name for people. */
  surface: string
  /** The source turn for an agent note. */
  agent?: AgentSource
}

/** Where each note is shown in a transcript (`placeNotes`). */
export type NotePlacement<N extends Pick<Note, "afterExecutionId">> = {
  /**
   * Before the first turn shown: left before any turn, or after a turn the
   * bounded view left out, which is older than every turn it keeps.
   */
  before: N[]
  /**
   * Right after the turn shown that was the last accepted when each was left,
   * in the order left.
   */
  byExecution: Map<string, N[]>
  /**
   * Right after the input still waiting that was the last accepted when each
   * was left, by its execution id, in the order left.
   */
  byWaiting: Map<string, N[]>
}

/**
 * Where the notes left in a conversation go among the turns shown.
 *
 * A note goes right after the turn that was the last the conversation had
 * accepted when it was left, and one whose turn is still an input waiting to
 * run right after that input. One left before any turn goes before
 * the first turn shown, and so does one whose turn is neither shown nor
 * waiting: that turn is one the bounded view left out, and every turn the view
 * leaves out is older than the ones it keeps, as `placePublished` holds too.
 * No note is dropped from view, and the order they were left in is kept within
 * each place (`notes.test.ts`).
 *
 * The one rule for every transcript that shows them, so the panel and the
 * desktop window cannot place the same note in two different places.
 */
export function placeNotes<N extends Pick<Note, "afterExecutionId">>(
  notes: readonly N[],
  shownExecutionIds: Iterable<string>,
  /**
   * The inputs still waiting to run that the transcript draws apart from its
   * turns. A turn shown is placed as a turn even when it is waiting too.
   */
  waitingExecutionIds: Iterable<string>,
): NotePlacement<N> {
  const shown = new Set(shownExecutionIds)
  const waiting = new Set(waitingExecutionIds)
  const placement: NotePlacement<N> = {
    before: [],
    byExecution: new Map(),
    byWaiting: new Map(),
  }
  for (const note of notes) {
    if (note.afterExecutionId === undefined) placement.before.push(note)
    else if (shown.has(note.afterExecutionId))
      placement.byExecution.set(note.afterExecutionId, [
        ...(placement.byExecution.get(note.afterExecutionId) ?? []),
        note,
      ])
    else if (waiting.has(note.afterExecutionId))
      placement.byWaiting.set(note.afterExecutionId, [
        ...(placement.byWaiting.get(note.afterExecutionId) ?? []),
        note,
      ])
    else placement.before.push(note)
  }
  return placement
}

/** Who left a note, as its label names them. */
const noteAuthors: Record<NoteAuthor["kind"], string> = {
  owner: "the owner",
  grant: "someone it is shared with",
  agent: "an agent",
}

/**
 * Who left a note, as a sentence: its words, and the surface — a name the
 * window did not choose — apart from them, so it is shown isolated from the
 * words around it.
 */
export function noteAuthorLabel(
  author: NoteAuthor,
): readonly (string | { readonly name: string })[] {
  if (!author.agent)
    return [`Note left by ${noteAuthors[author.kind]}, from `, { name: author.surface }]
  return [
    `Note left by ${noteAuthors[author.kind]}`,
    ` in chat `,
    { name: author.agent.conversationId },
    ", from ",
    { name: author.surface },
  ]
}

/** Why a retired note was given to no turn, in the label's words. */
const retiredReasons: Record<NonNullable<Note["retiredReason"]>, string> = {
  expired: "Not delivered: expired",
  permission_lost: "Not delivered: sharing changed",
}

/**
 * Where a note stands, in a few words; `null` for a note left only to be
 * read, which has nothing to wait for. A retired note the view gave no reason
 * for says only that it was not delivered.
 */
export function noteStateLabel(note: Pick<Note, "deliveryState" | "retiredReason">) {
  switch (note.deliveryState) {
    case "recorded":
      return null
    case "pending":
      return "Waiting for the next turn"
    case "assigned":
      return "Given to a turn"
    case "retired":
      return note.retiredReason === undefined
        ? "Not delivered"
        : retiredReasons[note.retiredReason]
  }
}
