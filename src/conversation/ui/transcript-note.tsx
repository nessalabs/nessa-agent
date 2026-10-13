import { Card } from "@nessa-ui/react/card"
import { StatusLabel, type StatusLabelTone } from "@nessa-ui/react/status-label"
import { noteAuthorLabel, noteStateLabel, type Note } from "../model"

/** How a note's state reads at a glance; the words say what it is. */
const stateTones: Record<Note["deliveryState"], StatusLabelTone> = {
  recorded: "neutral",
  pending: "active",
  assigned: "good",
  retired: "bad",
}

/**
 * A note somebody left in the conversation, as its own row: who left it and
 * from where, where it stands, and what it says. It is not a turn and not the
 * person's message, so it is no chat bubble — a quiet card between the turns.
 * What it says is shown as it was left, as plain text, never as markdown, and
 * a long word wraps rather than widening the transcript. The surface it was
 * left from is a name the window did not choose, so it is isolated from the
 * words around it.
 *
 * The one row for every transcript that shows notes: the panel's and the
 * desktop window's.
 */
export function TranscriptNote({ note }: { note: Note }) {
  const state = noteStateLabel(note)
  return (
    <Card
      role="note"
      data-note={note.id}
      data-note-state={note.deliveryState}
      data-note-author={note.author.kind}
      className="min-w-0 gap-1 rounded-lg px-3 py-2 shadow-none"
    >
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-0.5">
        <p
          data-note-label
          className="m-0 min-w-0 font-sans nessa-text-1 text-muted-foreground [overflow-wrap:anywhere]"
        >
          {noteAuthorLabel(note.author).map((piece, index) =>
            typeof piece === "string" ? piece : <bdi key={index}>{piece.name}</bdi>,
          )}
        </p>
        {state === null ? null : (
          <StatusLabel data-note-status tone={stateTones[note.deliveryState]}>
            {state}
          </StatusLabel>
        )}
      </div>
      <p
        data-note-text
        className="m-0 min-w-0 font-sans nessa-text-2 whitespace-pre-wrap [overflow-wrap:anywhere]"
      >
        {note.text}
      </p>
    </Card>
  )
}

/** The notes placed at one point of a transcript, in the order they were left; nothing for none. */
export function TranscriptNotes({ notes }: { notes: readonly Note[] }) {
  return notes.map((note) => <TranscriptNote key={note.id} note={note} />)
}
