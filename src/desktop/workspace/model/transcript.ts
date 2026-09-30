/**
 * A session's conversation: messages from the person and the agent, what the
 * agent is doing now, and the approval it is waiting on, if any. The agent's
 * messages are made of parts — prose, the steps it took, code, lists — and
 * the rules for reading them (grouping steps, inline code and emphasis, a
 * title from a first message) live here, not in the views.
 */
import type { WorkspaceFailureReason } from "./failure"

export type StepKind = "read" | "edit" | "run" | "search"

export type Part =
  | { readonly kind: "text"; readonly text: string }
  | {
      readonly kind: "step"
      readonly step: StepKind
      readonly label: string
      readonly detail?: string
      /** Lines added and removed, for an edit. */
      readonly added?: number
      readonly removed?: number
    }
  | { readonly kind: "code"; readonly code: string }
  | { readonly kind: "list"; readonly items: readonly string[] }
  /** Something the conversation carries besides words, drawn by the plugin it names. */
  | { readonly kind: "widget"; readonly plugin: string; readonly id: string }

export type StepPart = Extract<Part, { kind: "step" }>

/**
 * Whether a message the person wrote here has reached the source. Absent on
 * everything the source itself reported.
 */
export type Delivery =
  | { readonly state: "sending" }
  | { readonly state: "failed"; readonly reason: WorkspaceFailureReason }

export interface Message {
  readonly id: string
  readonly role: "user" | "agent"
  readonly at: number
  readonly parts: readonly Part[]
  readonly delivery?: Delivery
}

/** A command the agent asks to run, waiting on the person's answer. */
export interface Approval {
  readonly id: string
  readonly command: string
  readonly reason: string
}

/** What a running agent is doing, and since when; absent while its reply streams in. */
export interface Activity {
  readonly label: string
  readonly since: number
}

export interface Transcript {
  readonly sessionId: string
  readonly messages: readonly Message[]
  readonly activity: Activity | null
  readonly approval: Approval | null
  /** The source's count of changes to this conversation; see `revision.ts`. */
  readonly revision: number
}

/** A conversation with nothing in it, before the source has said anything of it. */
export function emptyTranscript(sessionId: string): Transcript {
  return { sessionId, messages: [], activity: null, approval: null, revision: 0 }
}

/**
 * The messages the person sent that `transcript` does not hold yet: they show
 * after it until the source's conversation includes them by id. The same list
 * when none was retired.
 */
export function unconfirmed(
  sent: readonly Message[],
  transcript: Transcript | undefined,
): readonly Message[] {
  if (!transcript) return sent
  const held = new Set(transcript.messages.map((message) => message.id))
  return sent.some((message) => held.has(message.id))
    ? sent.filter((message) => !held.has(message.id))
    : sent
}

/** A message's parts with consecutive steps gathered, so they read as one quiet group. */
export function groupSteps(parts: readonly Part[]): (Part | StepPart[])[] {
  const groups: (Part | StepPart[])[] = []
  for (const part of parts) {
    const last = groups[groups.length - 1]
    if (part.kind === "step" && Array.isArray(last)) last.push(part)
    else groups.push(part.kind === "step" ? [part] : part)
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
