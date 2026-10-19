import type { ModelReceipt } from "../model/model-choice"
import type { ShownWindow } from "../model/shown-window"
import type { FailureDiagnostic } from "../model/failure"
/**
 * What the workspace needs from whoever holds the sessions: the one port a
 * backend implements to attach. `adapters/gateway/` implements it over the
 * gateway's conversations, and `adapters/in-memory/` with sample data and
 * scripted replies; no view knows which.
 *
 * Every answer is the workspace's own types. Updates arrive as replacements —
 * a session's whole summary, a transcript's whole current state — each with
 * the source's revision of it (`model/revision.ts`), counted from 1, so
 * applying one twice, or out of step with a read, cannot duplicate or undo
 * anything: the newer revision wins whichever arrived first. A conversation
 * includes each message sent to it under the id it was sent with.
 *
 * A summary may say what is going on in its session now
 * (`SessionSummary.now`): the source's one-line account of its turn, which
 * the overview's peek shows as it is. It is part of the summary and follows
 * its revisions — a newer summary without one says there is nothing to say
 * — and it may lag or lead the conversation, which arrives on its own
 * revisions. The window shows nothing where the source says nothing; it
 * never writes one of its own.
 *
 * The stream of updates may lose some: a connection that drops, a listener
 * that joins late. Nothing here asks the source to replay them. A read of the
 * index is the resync instead: its sessions are every session the source
 * held when it answered, so one it no longer lists is taken out, and the
 * conversations on screen are read again (`loadWorkspace`). The source says
 * when it may have lost updates — it reconnected, or found a gap in its
 * stream — with a `resync` on the stream, and the window reads the index
 * again (`followWorkspace`). A source that cannot tell never sends one, and
 * the window then resyncs only when it is opened or asked to (Try Again).
 */
import type { StageMismatch, WorkspaceFailureReason } from "../model/failure"
import type { ModelRef, WorkspaceIndex, SessionSummary } from "../model/workspace-index"
import type { PaneRoom } from "../../split-panes/model/pane-sizing"
import type { QuestionChoice, Transcript } from "../model/transcript"
import type { AgentsFilter } from "../model/overview/filter"
import type {
  ShareGrant,
  ShareRequest,
  SharingChoices,
  SharingFailure,
} from "../model/sharing"
import type { QuestionFlights } from "./question-flights"
import type { AccessMode } from "../model/access"

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
  readonly start?: {
    readonly channelId: string
    readonly title: string
    /** The access chosen before it began; the source's default when absent. */
    readonly access?: AccessMode
  }
}

export type WorkspaceUpdate =
  /** A session began or changed; the summary replaces whatever was held. */
  | { readonly kind: "session"; readonly session: SessionSummary }
  /**
   * A session left the lists (archived). Its revision is counted with the
   * session's summary, not its conversation: the removal is the summary's
   * next revision, so a summary at a higher one — the session listed again —
   * outranks it, and one at or below it stays out.
   */
  | {
      readonly kind: "session-removed"
      readonly sessionId: string
      readonly revision: number
    }
  /** A session's conversation changed; the transcript replaces whatever was held. */
  | { readonly kind: "transcript"; readonly transcript: Transcript }
  /**
   * The stream may have lost updates — the source reconnected, or found a
   * gap in what it sent. It carries nothing to apply: the window reads the
   * index again, and every conversation on screen, to catch up.
   */
  | { readonly kind: "resync" }

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
  /**
   * Sections, channels and every session's summary: all the source holds when
   * it answers. A session it does not list is gone — the resync for updates
   * the stream lost.
   */
  index(): Promise<WorkspaceIndex>
  /** One session's conversation as it stands. */
  transcript(sessionId: string): Promise<Transcript>
  /** Changes to any session, until the returned function is called. */
  subscribe(listener: (update: WorkspaceUpdate) => void): () => void
  send(message: OutgoingMessage): Promise<void>
  /**
   * Allows, or refuses, what a session's agent waits to run. Like pinning and
   * archiving, a consequential call: the in-memory source records it — what
   * was asked, who asked it — as it begins to carry the call out, before the
   * effect, in the order calls are carried out, and then what became of it,
   * so a refused or failed one is on record too. The gateway's source records
   * nothing itself: what it sends is the gateway's to record, as this
   * window's authenticated caller (not person or agent), and what it refuses
   * without sending — an "always" answer other than the review's project
   * allow, a pin, a session taken out — is on
   * no record.
   * Resolves once the source has taken the answer; the conversation that no
   * longer asks reaches
   * subscribers as an update — before the call resolves where the source can
   * say it by then (the in-memory source always can), after it otherwise.
   * The window holds the answer as given until that conversation arrives
   * (`withTranscript` in `workspace-state.ts`).
   * `optionId` is the option the person chose. Two options may decide the
   * same way; the source submits this one, and refuses one the review does
   * not offer.
   */
  approve(
    sessionId: string,
    approvalId: string,
    scope: ApprovalScope,
    initiator: Initiator,
    optionId: string,
  ): Promise<void>
  deny(
    sessionId: string,
    approvalId: string,
    initiator: Initiator,
    optionId: string,
  ): Promise<void>
  /**
   * Reads again, now, a review or a question whose content could not be read
   * (`Approval.loading` or `TranscriptQuestion.loading` "failed"), named by
   * the approval's id or the question's key. A source that reads nothing
   * after a view leaves it out.
   */
  retryReview?(sessionId: string, approvalId: string): void
  /**
   * Answers a question the session's agent is waiting on, or declines it
   * (`choices` null). Resolves once the source has taken the answer. The
   * question leaves the conversation when a later read no longer lists it.
   * The gateway's source records nothing itself: the gateway records the
   * answer as this window's authenticated caller. `key` is the question's
   * own (`TranscriptQuestion.key`) where the source gave it one; a source
   * that keys its questions refuses an answer without it, or naming
   * another, as `not-waiting`.
   */
  answerQuestion(
    sessionId: string,
    executionId: string,
    questionId: string,
    choices: readonly QuestionChoice[] | null,
    initiator: Initiator,
    key?: string,
  ): Promise<void>
  /**
   * Stops the turn the session's agent is at work on (`atWork`): cancels it
   * where it runs, and lets go whatever it waits on. Refused `not-waiting`
   * when no turn is at work. Resolves once the source has taken it; the
   * conversation that no longer runs reaches subscribers as an update. Like
   * an answer, the gateway's source records nothing itself: the gateway
   * records the stop as this window's authenticated caller.
   */
  stop(sessionId: string, initiator: Initiator): Promise<void>
  /** Same-provider model control; the subscription owns confirmed display. */
  setModel(
    sessionId: string,
    model: ModelRef,
    requestId: string,
    initiator: Initiator,
  ): Promise<ModelReceipt>
  /**
   * Changes the approval preset the session's agent runs at (`access.ts`),
   * for every review decided after it. Resolves once the source has
   * committed it; the conversation saying so reaches subscribers as an
   * update. A source that cannot change it now refuses: the gateway changes
   * it only while no turn runs (`conversation.setApprovalMode`), and refuses
   * one the session's model does not offer. Like an answer, the gateway's
   * source records nothing itself: the gateway records the change, its
   * refusal included, as this window's authenticated caller.
   */
  setAccess(sessionId: string, mode: AccessMode, initiator: Initiator): Promise<void>
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

/** The source's refusal: a typed reason (`model/failure.ts`), never a sentence. */
export class WorkspaceSourceError extends Error {
  readonly reason: WorkspaceFailureReason
  readonly stages?: StageMismatch

  constructor(
    reason: WorkspaceFailureReason,
    stages?: StageMismatch,
    readonly diagnostic?: FailureDiagnostic,
  ) {
    super(`The workspace source refused: ${reason}`)
    this.name = "WorkspaceSourceError"
    this.reason = reason
    this.stages = stages
  }
}

/** A model control's uncertainty is preserved independently of its reason. */
export class WorkspaceModelError extends WorkspaceSourceError {
  constructor(
    reason: WorkspaceFailureReason,
    readonly uncertain: boolean,
    diagnostic?: FailureDiagnostic,
  ) {
    super(reason, undefined, diagnostic)
    this.name = "WorkspaceModelError"
  }
}

/**
 * Why a call to the source failed, by its type: the one place that tells a
 * refusal from a fault. Anything but the source's own typed refusal is a
 * fault, not an answer: it is logged as one, and taken as `unavailable`.
 */
export function failureReason(error: unknown): WorkspaceFailureReason {
  if (error instanceof WorkspaceSourceError) return error.reason
  console.error("The workspace source failed unexpectedly", error)
  return "unavailable"
}

/** Both stages, when a refusal is that the window and the server disagree. */
export function failureStages(error: unknown): StageMismatch | null {
  return error instanceof WorkspaceSourceError &&
    error.reason === "wrong-stage" &&
    error.stages
    ? error.stages
    : null
}

/**
 * Who a session is shared with, and sharing it (nessa_ui#128, #129): the
 * gateway's `conversation.share`, `.unshare` and `.shares`, the linked
 * devices it could be shared with, and the hosts the owner's turns may run
 * commands on. `adapters/gateway/sharing-gateway.ts` implements it over the
 * window's client, `adapters/in-memory/in-memory-sharing.ts` for the sample
 * and tests. Every call settles: it answers, or rejects with a typed
 * `SharingError`, `unanswered` when no answer came and it is not known
 * whether the change was made. A share or revoke is retried under the same
 * `requestId` only by asking again with it; the window does not, and reads
 * the grants again after either outcome instead. A share or revoke built on
 * a grant names its policy revision, and is refused `changed` when the grant
 * has moved since.
 */
export interface SharingSource {
  /** The session's grants, oldest first. */
  grants(sessionId: string): Promise<readonly ShareGrant[]>
  /** The devices a session may be shared with, and the hosts and bounds a Drive grant is held to. */
  choices(): Promise<SharingChoices>
  /**
   * Grant `request.role` to its device, or replace what that device holds.
   * Answers the grant's policy revision once the share is made.
   */
  share(sessionId: string, request: ShareRequest, requestId: string): Promise<number>
  /**
   * Take away what the device holds on the session, while it still holds
   * the grant at `expectedPolicyRevision`.
   */
  unshare(
    sessionId: string,
    credentialId: string,
    expectedPolicyRevision: number,
    requestId: string,
  ): Promise<void>
}

/** A sharing call's refusal, by kind (`model/sharing.ts`), never a sentence. */
export class SharingError extends Error {
  readonly reason: SharingFailure

  constructor(reason: SharingFailure) {
    super(`Sharing refused: ${reason}`)
    this.name = "SharingError"
    this.reason = reason
  }
}

/**
 * Why a sharing call failed, by its type. Anything but the source's own
 * typed refusal is a fault, logged as one and taken as `unanswered`.
 */
export function sharingFailure(error: unknown): SharingFailure {
  if (error instanceof SharingError) return error.reason
  console.error("Sharing failed unexpectedly", error)
  return "unanswered"
}

/**
 * What the workspace's commands are given, from composition: the source, and
 * the other outside things they need — the time, fresh ids for what the
 * person creates (a new session, a message), and the room the panes have on
 * the page. Tests pass their own.
 */
export interface WorkspaceDependencies {
  /** Opens the provider login flow; resolution confirms launch, not sign-in. */
  readonly providerLoginAvailable?: () => Promise<boolean>
  readonly signInToProvider?: (provider: "claude" | "codex") => Promise<void>
  readonly workspace: WorkspaceSource
  readonly now: () => number
  readonly newId: () => string
  /**
   * The room the panes have now, as laid out (`adapters/dom/measure.ts`), or
   * nothing when no workspace is on the page. Every command that changes the
   * layout asks it, whoever dispatches it, and with nothing measured places
   * nothing new.
   */
  readonly measure: () => PaneRoom | undefined
  /**
   * Where the Agents overview's filter is remembered between launches: read
   * once, into the store's first state, and written on each change
   * (`effects.ts`). The store owns the filter; this only keeps it.
   */
  readonly overviewFilter: RememberedFilter
  /**
   * Asks already on their way, for this composed store. A second card joins
   * the send when its choices and initiator match, and hears how it settled.
   * A different answer does not (`question-flights.ts`).
   */
  readonly questionFlights: QuestionFlights
  /**
   * Who a session is shared with, and sharing it. Absent where the window
   * has nothing to share through — a source composition was handed with no
   * gateway — and a session's menu then offers no Share.
   */
  readonly sharing?: SharingSource
  /**
   * Where what the window shows is told, for the person's agents to read
   * (`effects.ts`). Absent where nobody could read it: the sample workspace,
   * or a source composition was handed with no gateway.
   */
  readonly windowPublisher?: WindowPublisher
}

/**
 * Tells the gateway what the window shows. Fire and forget: the newest word
 * wins, it is said again whenever the window connects again, and a window
 * that could not say it shows the person nothing different.
 */
export interface WindowPublisher {
  /** What the window shows now, or `null` when it draws no panes. */
  publish(window: ShownWindow | null): void
}

/** A filter kept outside the window's state, between launches. */
export interface RememberedFilter {
  /** What was chosen last; the overview's default when nothing is kept, or storage refuses. */
  read(): AgentsFilter
  /** Keeps a choice; a refusal to keep it is survivable, and the choice still applies. */
  write(filter: AgentsFilter): void
}
