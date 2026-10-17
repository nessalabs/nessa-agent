/**
 * Access: the approval preset a session's agent runs at — ask before tools,
 * approve automatically, or full access (Claude's `bypassPermissions`). The
 * gateway owns it (`conversation.setApprovalMode`, `ConversationView.approvalMode`);
 * the composer's shield shows it and asks for another.
 *
 * Two facts, two owners. What the session runs at is the source's
 * (`SessionAccess`, on the transcript). What the person asked for waits beside
 * it (`AccessChoice`, in `WorkspaceState.accessChoices`) until the source's
 * conversation says the same, as an answer to an approval does. The shield
 * shows the choice only while it is the newer word — chosen for a session not
 * begun yet, on its way, or taken and not yet in a conversation — and the
 * source's mode otherwise, so a refused change never shows as made.
 */
import type { WorkspaceFailureReason } from "./failure"
import { atWork, type Transcript } from "./transcript"

export type AccessMode = "ask" | "auto" | "full"

/** The session's access, as the source last said it. */
export interface SessionAccess {
  /** The preset the source has committed. */
  readonly mode: AccessMode
  /** The presets the session's model offers, in the source's order. */
  readonly modes: readonly AccessMode[]
  /** A change the source has under way, or is recovering: not in effect yet. */
  readonly changing?: AccessMode
}

/**
 * The person's choice of access for a session. `chosen`: held for a session
 * the source has not begun, sent with its first message. `pending`: chosen
 * while the session's agent is at work, held here and asked of the source
 * once a conversation arrives with no turn at work — the gateway changes
 * access only between turns. `changing`: asked of the source. `taken`: the
 * source committed it, and no conversation has said so yet. `refused`: the
 * source did not take it; the source's mode shows, and the refusal is said
 * until the next conversation with no turn at work, or the next pick.
 */
export type AccessChoice =
  | {
      readonly mode: AccessMode
      readonly state: "chosen"
      /**
       * A first message on its way carries it to the source's create. A pick
       * made after that is not carried: the session begins at the captured
       * mode, and the later pick is the person's newer word.
       */
      readonly captured?: boolean
    }
  | {
      readonly mode: AccessMode
      readonly state: "changing" | "taken"
    }
  | {
      readonly mode: AccessMode
      readonly state: "pending"
      /**
       * Sent only on a conversation newer than this revision: set when the
       * source refused it because a turn had begun first, so it waits for that
       * turn to end rather than being sent again on the conversation held.
       */
      readonly after?: number
    }
  | {
      readonly mode: AccessMode
      readonly state: "refused"
      readonly reason: WorkspaceFailureReason
      /** The source's own code for the refusal, when it gave one (`turn_running`). */
      readonly code?: string
    }

/** Every session begins asking, as the gateway's create does when told nothing. */
export const defaultAccess: AccessMode = "ask"

/** What the shield shows: the choice while it is the newer word, else the source's mode. */
export function shownAccess(
  source: SessionAccess | undefined,
  choice: AccessChoice | undefined,
): AccessMode {
  if (choice && choice.state !== "refused") return choice.mode
  return source?.mode ?? defaultAccess
}

/**
 * The choice once the source's conversation `transcript` has arrived: let go
 * when that conversation says what it asked for. A session's first choice,
 * captured by its first message, is let go once the conversation says any
 * access at all, since a create the gateway reopened keeps its own preset.
 * One made after that capture is the person's newer word: it becomes
 * pending, and is asked of the source once no turn is at work — or is let go
 * when the source already runs it. A refusal is let go by a conversation
 * with no turn at work: what it refused is history. One pending or on its
 * way is kept: the pending one is sent (`effects.ts`), and the call settles
 * the other.
 */
export function settledBy(
  choice: AccessChoice | undefined,
  transcript: Transcript,
): AccessChoice | undefined {
  const source = transcript.access
  if (!choice) return choice
  if (choice.state === "refused") return atWork(transcript) ? choice : undefined
  if (!source) return choice
  if (choice.state === "chosen")
    return choice.captured || source.mode === choice.mode
      ? undefined
      : { mode: choice.mode, state: "pending" }
  if (choice.state === "taken" && source.mode === choice.mode) return undefined
  return choice
}
