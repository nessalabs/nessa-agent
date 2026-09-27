/**
 * What the workspace needs from whoever holds the sessions: the one port a
 * backend implements to attach. Today `adapters/in-memory/` implements it with
 * sample data and scripted replies; the gateway will implement it next, and
 * no view changes when it does.
 *
 * Every answer is the workspace's own types. Updates arrive as replacements —
 * a session's whole summary, a transcript's whole current state — each with
 * the source's revision of it (`model/revision.ts`), counted from 1, so
 * applying one twice, or out of step with a read, cannot duplicate or undo
 * anything: the newer revision wins whichever arrived first. A conversation
 * includes each message sent to it under the id it was sent with.
 */
import type { ModelRef, Organisation, SessionSummary } from "../model/organisation"
import type { Transcript } from "../model/transcript"

/**
 * A message sent to a session, as the source is asked to deliver it. Sent
 * while an approval waits, it moves the turn on and lets the approval go —
 * a consequential change the source records, with who sent it.
 */
export interface OutgoingMessage {
  readonly sessionId: string
  /** Who sent it: the person at the composer, or an agent. */
  readonly initiator: Initiator
  /**
   * The message's identity, minted when it was written; the source keeps it.
   * Sending an id the source already took does nothing: a message sent again
   * after a refusal that was not one is taken once.
   */
  readonly messageId: string
  readonly text: string
  /** The model the turn runs on, chosen in the composer. */
  readonly model: ModelRef
  /**
   * Present while the source has not spoken of the session — its first
   * message, one sent before the source answered the first, or one sent after
   * the first was refused: the session begins with it, under this id. Taking
   * it is idempotent: a session that has begun is not begun again, and the
   * message joins it.
   */
  readonly start?: { readonly channelId: string; readonly title: string }
}

export type WorkspaceUpdate =
  /** A session began or changed; the summary replaces whatever was held. */
  | { readonly kind: "session"; readonly session: SessionSummary }
  /** A session left the lists (archived), at this revision of it. */
  | {
      readonly kind: "session-removed"
      readonly sessionId: string
      readonly revision: number
    }
  /** A session's conversation changed; the transcript replaces whatever was held. */
  | { readonly kind: "transcript"; readonly transcript: Transcript }

/** How the person answered an approval. */
export type ApprovalScope = "once" | "always"

/**
 * Who made a decision the source is asked to carry out: the person, by the
 * window's own controls, or an agent dispatching the same command.
 */
export type Initiator = "person" | "agent"

/**
 * Every call settles: it answers, or rejects — with a typed
 * `WorkspaceSourceError` when the source refuses, and on a timeout of the
 * adapter's own when it cannot tell. The window holds nothing open for a call
 * that never settles; a read or a send left hanging stays "reading" or
 * "sending" for good.
 */
export interface WorkspaceSource {
  /** Sections, channels and every session's summary. */
  organisation(): Promise<Organisation>
  /** One session's conversation as it stands. */
  transcript(sessionId: string): Promise<Transcript>
  /** Changes to any session, until the returned function is called. */
  subscribe(listener: (update: WorkspaceUpdate) => void): () => void
  send(message: OutgoingMessage): Promise<void>
  /**
   * Allows, or refuses, what a session's agent waits to run. Like pinning and
   * archiving, a consequential call: the source records it — what was asked,
   * who asked it — before it carries it out, and then what became of it, so a
   * refused or failed one is on record too. Resolves once the conversation
   * that no longer asks has reached subscribers.
   */
  approve(
    sessionId: string,
    approvalId: string,
    scope: ApprovalScope,
    initiator: Initiator,
  ): Promise<void>
  deny(sessionId: string, approvalId: string, initiator: Initiator): Promise<void>
  /**
   * Pins or unpins a session. Resolves once the session's changed summary has
   * reached subscribers: the window shows the pin from that update alone.
   */
  setPinned(sessionId: string, pinned: boolean, initiator: Initiator): Promise<void>
  /**
   * Archives a session. Resolves once its `session-removed` update has reached
   * subscribers: the window takes the session out on that update alone, so a
   * removal has one path whoever asked for it.
   */
  archive(sessionId: string, initiator: Initiator): Promise<void>
  markRead(sessionId: string): Promise<void>
}

/**
 * Why the source did not do what it was asked. `unavailable`: no answer came —
 * the call may or may not have been done, so a message is sent again under
 * its id and the source's updates say what happened. `unknown-session`: it
 * holds no such session, archived ones included, and begins none under an
 * archived id. `not-waiting`: the approval was already answered, or never
 * asked.
 */
export type WorkspaceFailureReason = "unavailable" | "unknown-session" | "not-waiting"

export class WorkspaceSourceError extends Error {
  readonly reason: WorkspaceFailureReason

  constructor(reason: WorkspaceFailureReason) {
    super(failureText(reason))
    this.name = "WorkspaceSourceError"
    this.reason = reason
  }
}

/** The sentence a person is shown for each failure. */
export function failureText(reason: WorkspaceFailureReason): string {
  switch (reason) {
    case "unavailable":
      return "Nessa couldn’t reach your sessions."
    case "unknown-session":
      return "This session is no longer there."
    case "not-waiting":
      return "This was already answered."
  }
}

/** The failure a rejected call carries, by its type; anything else is taken as unreachable. */
export function failureReason(error: unknown): WorkspaceFailureReason {
  return error instanceof WorkspaceSourceError ? error.reason : "unavailable"
}

/**
 * What the workspace's commands are given, from composition: the source, and
 * the two other outside things they need — the time, and fresh ids for what
 * the person creates (a new session, a message). Tests pass their own.
 */
export interface WorkspaceDependencies {
  readonly workspace: WorkspaceSource
  readonly now: () => number
  readonly newId: () => string
}
