import type { ConversationView } from "@nessa/client"
import type { Note } from "../../model"

/**
 * A note in the gateway's view as a transcript shows it: what its row draws,
 * and the turn it is shown after. When it was accepted, what it was left for
 * (its state says), and the turn that took it are the gateway's record, not a
 * transcript's. The one reading of a note, for the panel's view and the
 * desktop window's alike.
 */
export function shownNote(note: ConversationView["notes"][number]): Note {
  return {
    id: note.messageId,
    text: note.text,
    author: {
      kind: note.author.kind,
      surface: note.author.surface,
      ...(note.author.agent ? { agent: note.author.agent } : {}),
    },
    deliveryState: note.deliveryState,
    ...(note.retiredReason === undefined ? {} : { retiredReason: note.retiredReason }),
    ...(note.afterExecutionId === undefined
      ? {}
      : { afterExecutionId: note.afterExecutionId }),
  }
}
