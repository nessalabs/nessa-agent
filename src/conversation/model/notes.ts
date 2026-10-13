/**
 * A note somebody left in a conversation, as its view lists it: the owner, or
 * someone the conversation is shared with under their grant. A note is not a
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

/** Who left a note: the owner, or someone it is shared with, and from which surface. */
export type NoteAuthor = {
  kind: "owner" | "grant"
  /** The surface the gateway verified it was left from, a name for people. */
  surface: string
}

/** Where each note is shown in a transcript (`placeNotes`). */
export type NotePlacement<N extends Pick<Note, "afterExecutionId">> = {
  /** Left before any turn: before the first turn shown. */
  before: N[]
  /** Right after the turn shown that was the last accepted when each was left, in the order left. */
  byExecution: Map<string, N[]>
  /** Left after a turn that is not shown: after the last one shown. */
  after: N[]
}

/**
 * Where the notes left in a conversation go among the turns shown.
 *
 * A note goes right after the turn that was the last the conversation had
 * accepted when it was left. One left before any turn goes before the first
 * turn shown. One whose turn is not shown — not listed yet, or no longer —
 * goes after the last turn shown, so it is not dropped from view nor put
 * ahead of a turn it followed (`notes.test.ts`). The order they were left in is kept within
 * each place.
 *
 * The one rule for every transcript that shows them, so the panel and the
 * desktop window cannot place the same note in two different places.
 */
export function placeNotes<N extends Pick<Note, "afterExecutionId">>(
  notes: readonly N[],
  shownExecutionIds: Iterable<string>,
): NotePlacement<N> {
  const shown = new Set(shownExecutionIds)
  const placement: NotePlacement<N> = { before: [], byExecution: new Map(), after: [] }
  for (const note of notes) {
    if (note.afterExecutionId === undefined) placement.before.push(note)
    else if (!shown.has(note.afterExecutionId)) placement.after.push(note)
    else
      placement.byExecution.set(note.afterExecutionId, [
        ...(placement.byExecution.get(note.afterExecutionId) ?? []),
        note,
      ])
  }
  return placement
}

/** Who left a note, as its label names them. */
const noteAuthors: Record<NoteAuthor["kind"], string> = {
  owner: "the owner",
  grant: "someone it is shared with",
}

/**
 * Who left a note, as a sentence: its words, and the surface — a name the
 * window did not choose — apart from them, so it is shown isolated from the
 * words around it.
 */
export function noteAuthorLabel(
  author: NoteAuthor,
): readonly (string | { readonly name: string })[] {
  return [`Note left by ${noteAuthors[author.kind]}, from `, { name: author.surface }]
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
      return "Given to the next turn"
    case "retired":
      return note.retiredReason === undefined
        ? "Not delivered"
        : retiredReasons[note.retiredReason]
  }
}
