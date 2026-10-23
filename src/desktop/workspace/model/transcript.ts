import type { ModelControl } from "./model-choice"
import type { ModelRef } from "./workspace-index"
import type { FailureDiagnostic } from "./failure"
import type { SessionAccess } from "./access"
/**
 * A session's conversation: messages from the person and the agent, what the
 * agent is doing now, and the approvals it is waiting on, if any. The agent's
 * messages are made of parts — prose, the steps it took, code, lists — and
 * the rules for reading them (grouping steps, inline code and emphasis, a
 * title from a first message) live here, not in the views.
 */
import { appWidget } from "../../widgets/app/model/app-ref"
import type { WorkspaceFailureReason } from "./failure"
import type { WidgetRef } from "../../widgets/model/widget-ref"
import type {
  HistoryExtent,
  Note,
  ImageReferencePart,
  LinkedFileReferencePart,
  UsageLimitNotice,
} from "../../../conversation"

export type StepKind = "read" | "edit" | "run" | "search"
export type StepStatus = "pending" | "running" | "completed" | "failed" | "stopped"
export type StepDiff = {
  readonly filename: string
  readonly from: string
  readonly to: string
}

export type Part =
  | { readonly kind: "text"; readonly text: string }
  /** Sent inputs retain references, distinct from files the agent published. */
  | {
      readonly kind: "attachments"
      readonly content: readonly (ImageReferencePart | LinkedFileReferencePart)[]
    }
  | {
      readonly kind: "step"
      readonly step: StepKind
      readonly label: string
      readonly detail?: string
      /** Lines added and removed, for an edit. */
      readonly added?: number
      readonly removed?: number
      /** The tool call, the same across its updates. */
      readonly id?: string
      /**
       * Where the call is. A call its turn ended under before it settled is
       * `stopped`. Absent where nothing said; read as finished once the turn is.
       */
      readonly status?: StepStatus
      /** When it started and settled, in Unix milliseconds on this device's clock. */
      readonly startedAtMs?: number
      readonly endedAtMs?: number
      /** What the call was given: the agent's arguments, as JSON text. */
      readonly input?: string
      /** What the call gave back. */
      readonly output?: string
      /** What an edit changed: the file before and after. */
      readonly diff?: StepDiff
    }
  | { readonly kind: "code"; readonly code: string }
  | { readonly kind: "list"; readonly items: readonly string[] }
  /**
   * A view a plugin draws in the message (ADR 326), such as an MCP app's UI
   * for the tool call that produced it: a card `InlineWidget` draws
   * (`ui/transcript/message.tsx`). The readers of a message's text and steps
   * pass over it, and the overview's peek drops it (`model/overview/peek.ts`).
   */
  | { readonly kind: "widget"; readonly widget: WidgetRef }
  /**
   * A file the agent published, which the conversation holds: by its name, for
   * people, and by exactly what the conversation holds it as — its SHA-256,
   * media type and size — which is what reads its bytes back. A card shows it
   * (`PublishedFiles`, `ui/transcript/message.tsx`); like a widget, the
   * readers of a message's text pass over it, and the overview's peek drops it.
   */
  | {
      readonly kind: "file"
      readonly name: string
      readonly digest: string
      readonly mimeType: string
      readonly size: number
    }

export type StepPart = Extract<Part, { kind: "step" }>
export type FilePart = Extract<Part, { kind: "file" }>
/** Files published one after another in a message, shown as one list. */
export type FileGroup = { readonly kind: "files"; readonly files: FilePart[] }

/**
 * Whether a message the person wrote here has reached the source. Absent on
 * everything the source itself reported.
 */
export type Delivery =
  | { readonly state: "sending" }
  | {
      readonly state: "failed"
      readonly reason: WorkspaceFailureReason
      readonly diagnostic?: FailureDiagnostic
      readonly model?: ModelRef
    }

/**
 * How an admitted turn ended, as the gateway reported it, when it did not
 * simply finish or keep running. Separate from `Delivery`, which is only
 * whether a message written in this window reached the source.
 */
export type TurnTerminalStatus = "failed" | "cancelled" | "unresolved"

export interface TurnTerminal {
  readonly status: TurnTerminalStatus
  /** The gateway's diagnostic, when it sent one. Shown as it arrived. */
  readonly error?: string
}

/** The status word, for the mark beside a turn. The diagnostic stays `error`. */
export function terminalLabel(status: TurnTerminalStatus): string {
  switch (status) {
    case "failed":
      return "Failed"
    case "cancelled":
      return "Cancelled"
    case "unresolved":
      return "Unresolved"
  }
}

/**
 * One thing a question asks, in the agent's own words. Answering it
 * authorises nothing.
 */
export interface TranscriptAsked {
  readonly key: string
  readonly prompt: string
  readonly header?: string
  readonly multiSelect: boolean
  readonly freeText: boolean
  readonly required: boolean
  readonly options: readonly {
    readonly value: string
    readonly label: string
    readonly description?: string
  }[]
}

/** A question the agent is waiting on. Identity is the execution and the ask. */
export interface TranscriptQuestion {
  /**
   * The source's own key for this question, when it has one: an answer names
   * it, so a question asked again under the same identities is never
   * answered by mistake.
   */
  readonly key?: string
  /**
   * Present while all it asks is still being read (`reading`), or could not
   * be (`failed`): it offers no answer, and its identities and fields are
   * not known yet.
   */
  readonly loading?: "reading" | "failed"
  readonly executionId: string
  readonly questionId: string
  readonly message: string
  readonly questions: readonly TranscriptAsked[]
}

/** What was chosen for one field of a question. A null list declines the ask. */
export interface QuestionChoice {
  readonly key: string
  readonly values: readonly string[]
  readonly ownWords?: string
}

/**
 * One row of a conversation: a message from the person or the agent, a
 * note somebody left in it (`NoteMessage`), a saved record this read
 * could not apply (`UnreadableMessage`), or the line where its agent ran
 * on in a new session (`HandoffMessage`).
 */
export type Message =
  | TurnMessage
  | NoteMessage
  | UnreadableMessage
  | UnreadableOverflowMessage
  | HandoffMessage

/** A message the person or the agent said in a turn. */
export interface TurnMessage {
  readonly id: string
  readonly role: "user" | "agent"
  readonly at: number
  readonly parts: readonly Part[]
  /** Latest published execution observed when this local message began. */
  readonly observedInput?: string | null
  readonly delivery?: Delivery
  /**
   * Set on the agent's reply while its turn still runs or waits to, as the
   * source reported the turn. Absent once the turn ended, however it ended:
   * a settled reply folds each run of its steps into one line
   * (`ui/transcript/tool-steps.tsx`).
   */
  readonly working?: true
  /**
   * How this turn ended, when the gateway said it ended other than by
   * finishing. On the agent's reply when there is one, otherwise on the
   * input, so a turn that said nothing still shows it.
   */
  readonly terminal?: TurnTerminal
  /**
   * The execution this terminal belongs to, so a surface can match it to
   * `authenticationRefusal`. Set only with `terminal`.
   */
  readonly executionId?: string
  /**
   * Independent required work failed on this authentication refusal.
   * The pane keeps `terminal` beside the sign-in card when this is set.
   */
  readonly requiredWorkFailed?: boolean
  /**
   * A provider usage limit on this turn. The pane shows the card for the
   * latest turn that carries one, and hides a pure limit's failed mark.
   */
  readonly usageLimit?: UsageLimitNotice
  /**
   * The MCP App that wrote a message of the person's on their behalf
   * (`ui/message`, #390); absent when the person wrote it.
   */
  readonly app?: MessageApp
  readonly note?: never
}

/**
 * A note the owner, or someone the conversation is shared with, left in it:
 * not a turn and not the person's message, so it is drawn as a row of its
 * own (`TranscriptNote`), with who left it and where it stands. Nothing the window sends is a note, so it is never unsent.
 */
export interface NoteMessage {
  readonly id: string
  readonly role: "note"
  readonly at: number
  /** None: what it says is the note's. */
  readonly parts: readonly Part[]
  readonly note: Note
  readonly observedInput?: never
  readonly delivery?: never
  readonly app?: never
  readonly unreadable?: never
}

/**
 * A saved record this read could not apply, drawn as one line
 * (`UnreadablePartLine`). `unreadable.id` is the record's position, and the
 * line is where a future report action attaches. Nothing the window sends
 * is one of these.
 */
export interface UnreadableMessage {
  readonly id: string
  readonly role: "unreadable"
  readonly at: number
  readonly parts: readonly Part[]
  readonly unreadable: { readonly id: string; readonly afterExecutionId?: string }
  readonly note?: never
  readonly observedInput?: never
  readonly delivery?: never
  readonly app?: never
  /** A gap is not a turn, so it has no ending the gateway reported. */
  readonly terminal?: never
  readonly executionId?: never
  readonly requiredWorkFailed?: never
}

/**
 * Where the conversation's agent ran on in a new session that was given the
 * turns before it as text: a switch to another agent and model, or the fork
 * the conversation began as. Drawn as one line across the transcript
 * (`HandoffLine`). Nothing the window sends is one of these.
 */
export interface HandoffMessage {
  readonly id: string
  readonly role: "handoff"
  readonly at: number
  readonly parts: readonly Part[]
  readonly handoff: Handoff
  readonly note?: never
  readonly unreadable?: never
  readonly observedInput?: never
  readonly delivery?: never
  readonly app?: never
  /** A handoff is not a turn, so it has no ending the gateway reported. */
  readonly terminal?: never
  readonly executionId?: never
  readonly requiredWorkFailed?: never
}

/**
 * A handoff as the transcript draws it. A switch names the agent and model
 * either side of it; a fork, whose copied turns are drawn above it, says how
 * many it left out.
 */
export type Handoff =
  | { readonly kind: "switch"; readonly from: HandoffAgent; readonly to: HandoffAgent }
  | { readonly kind: "fork"; readonly omittedTurns: number }

/** An agent and model a switch left or went to; no agent for a provider Nessa has none for. */
export interface HandoffAgent {
  readonly agent?: string
  readonly model: string
}

/**
 * The final line when more records could not be read than the view lists
 * (`UnreadableOverflowLine`). Not a record, and not a turn.
 */
export interface UnreadableOverflowMessage {
  readonly id: string
  readonly role: "unreadable-overflow"
  readonly at: number
  readonly parts: readonly Part[]
  readonly note?: never
  readonly unreadable?: never
  readonly observedInput?: never
  readonly delivery?: never
  readonly app?: never
  /** The overflow line is not a turn, so it has no ending the gateway reported. */
  readonly terminal?: never
  readonly executionId?: never
  readonly requiredWorkFailed?: never
}

/** An MCP App, as a message it wrote names it: its server, and the tool whose UI it is. */
export interface MessageApp {
  readonly server: string
  readonly tool: string
}

/**
 * Who asks for an approval: the agent, or an MCP App, which names the tool
 * it asked to call on its own server.
 */
export type ApprovalOrigin =
  | { readonly kind: "agent" }
  | { readonly kind: "app"; readonly server: string; readonly tool: string }

/**
 * An answer a review offers, as its button gives it. `once` allows this
 * request; `always` reaches further. A gateway review has no `always`: the
 * projection offers no choice that reaches past its request
 * (`a_review_reaching_beyond_its_request_is_not_offered`).
 */
export type ApprovalChoice = "deny" | "always" | "once"

/** One answer a review offers. The card draws these and no others. */
export interface ApprovalOption {
  readonly id: string
  /** What the button says: the review's own words for this answer. */
  readonly label: string
  readonly choice: ApprovalChoice
}

/**
 * What an approval asks the person to allow: running a command or a tool,
 * or an MCP App sending one message in the conversation as them (#390).
 */
export type ApprovalAsk = "tool" | "message"

/**
 * What happens if nobody answers. `reason` is the past-tense record; the
 * card builds its line from `dueAtMs`, `effect`, and `stopsTurn`.
 */
export interface ApprovalDeadline {
  readonly waitMs: number
  /**
   * Unix milliseconds when the deadline decides, on this window's clock: put
   * there from the gateway's as the view arrived (`transcriptFrom`). A reload
   * counts down from this.
   */
  readonly dueAtMs: number
  readonly effect: "allow" | "deny"
  readonly reason: string
  /** The only refusal offered stops the turn. */
  readonly stopsTurn: boolean
}

/**
 * What the agent, or an MCP App, asks to do, waiting on the person's answer:
 * run a command or a tool, or — an app — send a message as them, `command`
 * then being the app's tool and the message's exact words.
 */
export interface Approval {
  readonly id: string
  readonly command: string
  /**
   * Present while what the review shows is still being read from its live
   * owner ("reading"), or could not be and is being read again ("failed"):
   * `command` is then only the tool's name, and no answer is offered until
   * it is read whole (`shownWhole`).
   */
  readonly loading?: "reading" | "failed"
  readonly reason: string
  readonly origin: ApprovalOrigin
  /** The answers this review offers, in the order it offers them. */
  readonly options: readonly ApprovalOption[]
  /**
   * The gateway's own word for what is asked, a closed set the client
   * holds the view to (`conversation-validate.ts`, D19 on #390).
   */
  readonly ask: ApprovalAsk
  /** Present when this review answers itself if nobody does. */
  readonly deadline?: ApprovalDeadline
}

/** Whether `approval` offers `choice`. A button and an answer both ask this. */
export function offersChoice(approval: Approval, choice: ApprovalChoice): boolean {
  return approval.options.some((option) => option.choice === choice)
}

/** Whether `approval` can be answered: all of what it allows is shown. */
export function shownWhole(approval: Approval): boolean {
  return approval.loading === undefined
}

/**
 * The first option of `choice`. A chord names a choice, not a button, so it
 * answers this one. A click answers the option on the button, which may be
 * a later one of the same choice.
 */
export function optionOf(
  approval: Approval,
  choice: ApprovalChoice,
): ApprovalOption | undefined {
  return approval.options.find((option) => option.choice === choice)
}

/**
 * Where a conversation's agent runs, from its latest run's lease (ADR 252),
 * in the gateway's own words: whether it runs, why it ended, or why it could
 * not start. `unreadable` is a lease the gateway cannot read.
 */
export interface TranscriptLease {
  readonly state: "live" | "ending" | "ended" | "interrupted" | "refused" | "unreadable"
  /** `here` is the gateway's own machine; `ssh` a host reached over SSH;
   * `peer` a paired machine that dials the gateway. */
  readonly environment?: "here" | "ssh" | "peer"
  /** The SSH destination, or the paired machine's `peer:<key>`, exactly when
   * `environment` is `ssh` or `peer`. */
  readonly host?: string
  readonly cause?: "stopped" | "closed" | "revoked" | "expired" | "lost" | "idle"
  readonly refusal?:
    | "sandbox_unavailable"
    | "environment_unreachable"
    | "environment_version_mismatch"
    | "environment_busy"
    | "environment_platform_unsupported"
    | "environment_install_failed"
    | "agent_unavailable"
}

/** What a running agent is doing, and since when; absent while its reply streams in. */
export interface Activity {
  readonly label: string
  readonly since: number
}

export interface Transcript {
  readonly modelControl?: ModelControl
  readonly sessionId: string
  readonly messages: readonly Message[]
  readonly activity: Activity | null
  /**
   * Every review the agent waits on, in the order the source lists them.
   * Each is shown and answered on its own: one that cannot be read yet
   * (`loading`) holds back none of the others.
   */
  readonly approvals: readonly Approval[]
  /**
   * What the bounded view left out (`historyExtent`). Absent when the source
   * did not say, or said this is the whole confirmed history.
   */
  readonly history?: HistoryExtent
  /** A provider's typed authentication refusal on the latest turn. */
  readonly authenticationRefusal?: string
  /**
   * Whether this gateway schedules a continuation when a usage limit resets.
   * Shared by every conversation. Absent when the source did not say.
   */
  readonly usageLimitAutoContinue?: boolean
  /**
   * Questions the agent is waiting on, as the view listed them. Empty when
   * it asked none that could be shown.
   */
  readonly questions?: readonly TranscriptQuestion[]
  /**
   * The gateway's notice that a review or question was left out of the view.
   * The view's own words; absent when it sent none.
   */
  readonly interactionNotice?: string
  /** Raw execution identity of the latest published input, including pending. */
  readonly latestInputId?: string
  /** Authoritative runtime agent; never inferred from the model catalogue. */
  readonly agent?: string
  /** Where its agent runs; absent until the gateway has recorded a lease. */
  readonly lease?: TranscriptLease
  /** The approval preset its agent runs at; absent where the source did not say (`access.ts`). */
  readonly access?: SessionAccess
  /** The source's count of changes to this conversation; see `revision.ts`. */
  readonly revision: number
}

/**
 * Whether the conversation's agent is at work on a turn, as the source
 * reported it: thinking or running a step (`activity`), writing a reply
 * (`working`), or holding its turn on the person's answer (an approval or a
 * question). This is what Stop is offered on, so it reads every one of the
 * source's marks of a turn in progress, and nothing else: not the session
 * list's status, not whether the view left a review out.
 */
export function atWork(transcript: Transcript | undefined): boolean {
  if (!transcript) return false
  return (
    transcript.activity !== null ||
    transcript.approvals.length > 0 ||
    (transcript.questions?.length ?? 0) > 0 ||
    transcript.messages.some(
      (message) => "working" in message && message.working === true,
    )
  )
}

/** A conversation with nothing in it, before the source has said anything of it. */
export function emptyTranscript(sessionId: string): Transcript {
  return { sessionId, messages: [], activity: null, approvals: [], revision: 0 }
}

/**
 * The messages the person sent that `transcript` does not hold yet: they show
 * after it until the source's conversation includes them by id. The same list
 * when none was retired.
 */
export function unconfirmed<Sent extends Message>(
  sent: readonly Sent[],
  transcript: Transcript | undefined,
): readonly Sent[] {
  if (!transcript) return sent
  const held = new Set(transcript.messages.map((message) => message.id))
  return sent.some((message) => held.has(message.id))
    ? sent.filter((message) => !held.has(message.id))
    : sent
}

/**
 * A message's parts with consecutive steps gathered, so they read as one quiet
 * group, and consecutive published files gathered, so they read as one list.
 * Anything else — prose, code, a widget — ends a group.
 */
export function groupSteps(
  parts: readonly Part[],
): (Exclude<Part, FilePart> | StepPart[] | FileGroup)[] {
  const groups: (Exclude<Part, FilePart> | StepPart[] | FileGroup)[] = []
  for (const part of parts) {
    const last = groups[groups.length - 1]
    if (part.kind === "step") {
      if (Array.isArray(last)) last.push(part)
      else groups.push([part])
    } else if (part.kind === "file") {
      if (last && !Array.isArray(last) && last.kind === "files") last.files.push(part)
      else groups.push({ kind: "files", files: [part] })
    } else groups.push(part)
  }
  return groups
}

export type InlineRun =
  | { readonly kind: "text"; readonly text: string }
  | { readonly kind: "code"; readonly text: string }
  | { readonly kind: "strong"; readonly text: string }

/** Prose with `inline code` and **strong** marked out; everything else is plain text. */
export function inlineRuns(text: string): InlineRun[] {
  return text
    .split(/(`[^`]+`|\*\*[^*]+\*\*)/g)
    .filter((piece) => piece !== "")
    .map((piece): InlineRun => {
      if (piece.length > 2 && piece.startsWith("`") && piece.endsWith("`"))
        return { kind: "code", text: piece.slice(1, -1) }
      if (piece.length > 4 && piece.startsWith("**") && piece.endsWith("**"))
        return { kind: "strong", text: piece.slice(2, -2) }
      return { kind: "text", text: piece }
    })
}

/** Whether two lines say the same thing, ignoring case, spacing and end punctuation. */
export function sameWords(a: string, b: string): boolean {
  const plain = (text: string) =>
    text
      .trim()
      .toLowerCase()
      .replace(/[.?!:,;…]+$/, "")
      .replace(/\s+/g, " ")
  return plain(a) === plain(b)
}

/** The longest title a first message is cut to, at a word. */
export const titleLength = 48

/** A session's title from its first message: the first sentence, kept short. */
export function titleFrom(text: string): string {
  const first = text
    .trim()
    .split(/\n|(?<=[.?!])\s/)[0]
    .replace(/[.?!:,;]+$/, "")
  const short =
    first.length <= titleLength
      ? first
      : `${first.slice(0, titleLength).replace(/\s+\S*$/, "")}…`
  return short.charAt(0).toUpperCase() + short.slice(1)
}

/** The plain text of a message, for a preview. */
export function messageText(message: Message): string {
  return message.parts
    .flatMap((part) =>
      part.kind === "text" ? [part.text] : part.kind === "list" ? part.items : [],
    )
    .join(" ")
}

/**
 * One tool call as the source reports it, for deciding whether a plugin draws
 * something for it: the call's identity, and — when the agent's harness said —
 * the MCP server and tool it went to and the UI resource the tool declared.
 */
export interface ToolCallIdentity {
  /** The session (conversation) the call was made in. */
  readonly sessionId: string
  readonly executionId: string
  readonly toolId: string
  readonly mcp?: {
    readonly server: string
    readonly tool: string
    readonly resourceUri?: string
  }
}

/**
 * The widget part a tool call's UI is drawn in, or `null` for a call whose
 * tool declared none, and for one whose harness did not say — the call's
 * steps and result read as they always have. The plugin is the MCP server's
 * app; the id is the call, by its session and the execution and tool
 * identities that name it there (`appWidget`, the widgets' one statement of
 * how an app's widgets are named).
 */
export function toolWidget(
  call: ToolCallIdentity,
): Extract<Part, { kind: "widget" }> | null {
  if (!call.mcp?.resourceUri) return null
  return {
    kind: "widget",
    widget: appWidget(call.mcp.server, call.sessionId, call.executionId, call.toolId),
  }
}
