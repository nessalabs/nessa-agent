/**
 * A workspace built during a run for a large-list measurement (#590).
 *
 * The window boots `sampleWorkspace`. This builder is not called from there.
 * It returns the same index and transcript shape `inMemorySource` already
 * accepts, with titles cut by `titleFrom`. Session ids are load-fixture ids,
 * not gateway conversation UUIDs. The builder does not call the gateway's
 * title, preview, list, or view owners, and it does not write a fixture file.
 */
import {
  composerModels,
  defaultComposerModel,
  type ComposerModel,
} from "../../../model/composer-options"
import {
  consistentIndex,
  type IndexContradictions,
  type ModelRef,
  type SessionStatus,
  type SessionSummary,
  type WorkspaceIndex,
} from "../../model/workspace-index"
import {
  titleFrom,
  type Activity,
  type Approval,
  type Message,
  type Part,
  type Transcript,
} from "../../model/transcript"
import { sampleApprovalOptions } from "./sample-content"

/** Why `seededWorkspace` refused a spec before building anything. */
export type SeededWorkspaceReason =
  | "seed"
  | "now"
  | "sessions"
  | "longTranscripts"
  | "messages"
  | "messageCharacters"
  | "catalogue"

/** A spec the builder will not run. The reason is the field that failed. */
export class SeededWorkspaceRefusal extends Error {
  readonly reason: SeededWorkspaceReason

  constructor(reason: SeededWorkspaceReason) {
    super(reason)
    this.name = "SeededWorkspaceRefusal"
    this.reason = reason
  }
}

/** What one run asks for. Counts are whole numbers. `seed` is a uint32. */
export interface SeededWorkspaceSpec {
  readonly seed: number
  /** Epoch milliseconds the timestamps are dated from. */
  readonly now: number
  readonly sessions: number
  /** How many of the newest sessions carry more than the opening message. */
  readonly longTranscripts: number
  /**
   * Messages in each of those transcripts, including the opening message.
   * Zero when `longTranscripts` is zero.
   */
  readonly messages: number
  /**
   * JavaScript string length of one plain-text part on the last message of
   * each long transcript. Zero unless `messages` is at least 2. The sentence
   * is ASCII, so this length is also its UTF-8 byte length.
   */
  readonly messageCharacters: number
}

/** Counts and sizes of one built workspace. Not a gateway limit. */
export interface SeededWorkspaceReport {
  readonly generator: "seeded-workspace"
  readonly algorithm: "mulberry32"
  readonly seed: number
  readonly now: number
  readonly sessions: number
  readonly channels: number
  readonly longTranscripts: number
  /** Messages stored on each long transcript. Zero when there are none. */
  readonly messages: number
  readonly maxTitleCharacters: number
  readonly maxPreviewUtf8Bytes: number
  readonly longPlainTextCharacters: number
  readonly longTranscriptUtf8Bytes: number
  readonly statusCounts: {
    readonly idle: number
    readonly running: number
    readonly needsYou: number
  }
  readonly largestChannelSessions: number
}

/** The index, every transcript, and the sizes of what was built. */
export interface SeededWorkspace {
  readonly index: WorkspaceIndex
  readonly transcripts: ReadonlyMap<string, Transcript>
  readonly contradictions: IndexContradictions
  readonly report: SeededWorkspaceReport
}

const sections = [
  { id: "load-starred", name: "Starred" },
  { id: "load-personal", name: "Personal" },
] as const

const channels = [
  {
    id: "load-desktop",
    name: "desktop",
    sectionId: "load-starred",
    private: false,
    topic: "Desktop window",
  },
  {
    id: "load-release",
    name: "release",
    sectionId: "load-starred",
    private: true,
    topic: "Release",
  },
  {
    id: "load-reading",
    name: "reading",
    sectionId: "load-personal",
    private: false,
    topic: "Reading",
  },
  {
    id: "load-home",
    name: "home",
    sectionId: "load-personal",
    private: false,
    topic: "Home",
  },
] as const

const words = [
  "backup",
  "channel",
  "export",
  "window",
  "transcript",
  "gateway",
  "preview",
  "session",
] as const

const runningLine = "Working through the transcript"
const markedLine = "The agent kept the **backup** in `export.ts`."
const fillerLine = "The next line repeats the same sentence."
const codeBlock = "function load(count) {\n  return count\n}\n"
const minute = 60_000
const uint32 = 0xffffffff
/** The inclusive range `Date` can format (`TimeClip` in ECMA-262). */
const dateLimit = 8_640_000_000_000_000

interface Cursor {
  readonly state: number
}

interface Draw {
  readonly cursor: Cursor
  readonly value: number
}

const whole = (value: number): boolean => Number.isInteger(value)

const refuse = (reason: SeededWorkspaceReason): never => {
  throw new SeededWorkspaceRefusal(reason)
}

function accepted(spec: SeededWorkspaceSpec): void {
  if (!whole(spec.seed) || spec.seed < 0 || spec.seed > uint32) refuse("seed")
  if (typeof spec.now !== "number" || !Number.isFinite(spec.now)) refuse("now")
  if (!whole(spec.sessions) || spec.sessions < 0) refuse("sessions")
  if (
    !whole(spec.longTranscripts) ||
    spec.longTranscripts < 0 ||
    spec.longTranscripts > spec.sessions
  )
    refuse("longTranscripts")
  if (!whole(spec.messages) || spec.messages < 0) refuse("messages")
  if (spec.longTranscripts === 0 ? spec.messages !== 0 : spec.messages < 2)
    refuse("messages")
  if (!whole(spec.messageCharacters) || spec.messageCharacters < 0)
    refuse("messageCharacters")
  if (spec.messageCharacters > 0 && spec.messages < 2) refuse("messageCharacters")
  if (!representableTime(spec.now) || !representableTime(earliestAt(spec))) refuse("now")
}

function representableTime(value: number): boolean {
  return Number.isFinite(value) && Math.abs(value) <= dateLimit
}

/**
 * The earliest instant the builder stores: the last session's start, or an
 * earlier long transcript when its messages reach back past an hour.
 */
function earliestAt(spec: SeededWorkspaceSpec): number {
  if (spec.sessions === 0) return spec.now
  const hour = 60
  const longLead = Math.max(hour, spec.messages - 1)
  const lastLead = spec.longTranscripts === spec.sessions ? longLead : hour
  let minutesBack = spec.sessions - 1 + lastLead
  if (spec.longTranscripts > 0 && spec.longTranscripts < spec.sessions) {
    const longBack = spec.longTranscripts - 1 + longLead
    if (longBack > minutesBack) minutesBack = longBack
  }
  return spec.now - minutesBack * minute
}

/** mulberry32. The returned value is a uint32; the cursor is the next state. */
function draw(cursor: Cursor): Draw {
  const state = (cursor.state + 0x6d2b79f5) >>> 0
  let mixed = Math.imul(state ^ (state >>> 15), state | 1)
  mixed = (mixed ^ (mixed + Math.imul(mixed ^ (mixed >>> 7), mixed | 61))) >>> 0
  return { cursor: { state }, value: (mixed ^ (mixed >>> 14)) >>> 0 }
}

function wordAt(cursor: Cursor): { readonly cursor: Cursor; readonly word: string } {
  const drawn = draw(cursor)
  return { cursor: drawn.cursor, word: words[drawn.value % words.length] ?? words[0] }
}

function opening(
  cursor: Cursor,
  index: number,
): { readonly cursor: Cursor; readonly text: string } {
  const first = wordAt(cursor)
  const second = wordAt(first.cursor)
  return {
    cursor: second.cursor,
    text: `please review the ${first.word} ${second.word} before export ${index}`,
  }
}

function longText(characters: number): string {
  const sentence = "The transcript line stays on the page. "
  if (characters === 0) return ""
  return sentence.repeat(Math.ceil(characters / sentence.length)).slice(0, characters)
}

function userMessage(id: string, at: number, text: string): Message {
  return { id, role: "user", at, parts: [{ kind: "text", text }] }
}

function agentTurn(id: string, at: number, extra: readonly Part[]): Message {
  const parts: Part[] = [
    { kind: "text", text: markedLine },
    { kind: "step", step: "read", label: "Read", detail: "export.ts" },
    { kind: "code", code: codeBlock },
    ...extra,
  ]
  return { id, role: "agent", at, parts }
}

function messagesFor(
  sessionId: string,
  at: number,
  text: string,
  count: number,
  characters: number,
): readonly Message[] {
  const messages: Message[] = []
  for (let index = 0; index < count; index += 1) {
    const id = `${sessionId}-${index + 1}`
    const when = at - (count - 1 - index) * minute
    if (index === 0) {
      messages.push(userMessage(id, when, text))
      continue
    }
    const last = index === count - 1
    const body = last ? longText(characters) : fillerLine
    messages.push(
      index % 2 === 1
        ? agentTurn(id, when, [{ kind: "text", text: body }])
        : userMessage(id, when, body),
    )
  }
  return messages
}

function transcriptUtf8Bytes(transcript: Transcript): number {
  const utf8 = new TextEncoder()
  let total = 0
  for (const message of transcript.messages)
    for (const part of message.parts) {
      if (part.kind === "text") total += utf8.encode(part.text).byteLength
      else if (part.kind === "code") total += utf8.encode(part.code).byteLength
      else if (part.kind === "list")
        for (const item of part.items) total += utf8.encode(item).byteLength
      else if (part.kind === "step") {
        total += utf8.encode(part.label).byteLength
        if (part.detail !== undefined) total += utf8.encode(part.detail).byteLength
      }
    }
  return total
}

function approvalFor(sessionId: string): Approval {
  return {
    id: `${sessionId}-approval`,
    command: "export the transcript",
    reason: "The export writes the session outside the workspace.",
    origin: { kind: "agent" },
    options: sampleApprovalOptions,
    ask: "tool",
  }
}

function transcriptFor(
  session: SessionSummary,
  text: string,
  count: number,
  characters: number,
): Transcript {
  const activity: Activity | null =
    session.status === "running" ? { label: runningLine, since: session.updatedAt } : null
  const approval = session.status === "needs-you" ? approvalFor(session.id) : null
  return {
    sessionId: session.id,
    messages: messagesFor(session.id, session.updatedAt, text, count, characters),
    activity,
    approval,
    revision: 1,
  }
}

function statusAt(value: number): SessionStatus {
  const roll = value % 10
  if (roll === 0) return "needs-you"
  if (roll <= 2) return "running"
  return "idle"
}

function sessionAt(
  index: number,
  cursor: Cursor,
  spec: SeededWorkspaceSpec,
  model: ModelRef,
): {
  readonly cursor: Cursor
  readonly session: SessionSummary
  readonly openingText: string
} {
  const drawn = draw(cursor)
  const status = statusAt(drawn.value)
  const pinnedDraw = draw(drawn.cursor)
  const unreadDraw = draw(pinnedDraw.cursor)
  const said = opening(unreadDraw.cursor, index)
  const updatedAt = spec.now - index * minute
  const session: SessionSummary = {
    id: `load-${String(index).padStart(5, "0")}`,
    channelId: channels[index % channels.length]?.id ?? channels[0].id,
    title: titleFrom(said.text),
    model,
    status,
    startedAt: updatedAt - 60 * minute,
    updatedAt,
    preview: said.text,
    ...(status === "running" ? { now: runningLine } : {}),
    pinned: pinnedDraw.value % 20 === 0,
    unread: unreadDraw.value % 4 === 0,
    revision: 1,
  }
  return { cursor: said.cursor, session, openingText: said.text }
}

function modelOf(models: readonly ComposerModel[]): ModelRef {
  const model = defaultComposerModel(models)
  if (model === undefined) throw new SeededWorkspaceRefusal("catalogue")
  return { provider: model.provider, modelId: model.modelId }
}

/** The last text part of the last message: what the session list previews. */
function lastSaid(transcript: Transcript): string {
  const last = transcript.messages[transcript.messages.length - 1]
  if (!last) return ""
  let text = ""
  for (const part of last.parts) if (part.kind === "text") text = part.text
  return text
}

function reportFor(
  spec: SeededWorkspaceSpec,
  sessions: readonly SessionSummary[],
  longTranscripts: readonly Transcript[],
): SeededWorkspaceReport {
  const utf8 = new TextEncoder()
  const statusCounts = { idle: 0, running: 0, needsYou: 0 }
  const channelCounts = new Map<string, number>()
  let maxTitleCharacters = 0
  let maxPreviewUtf8Bytes = 0
  for (const session of sessions) {
    if (session.status === "needs-you") statusCounts.needsYou += 1
    else statusCounts[session.status] += 1
    channelCounts.set(session.channelId, (channelCounts.get(session.channelId) ?? 0) + 1)
    if (session.title.length > maxTitleCharacters)
      maxTitleCharacters = session.title.length
    const previewBytes = utf8.encode(session.preview).byteLength
    if (previewBytes > maxPreviewUtf8Bytes) maxPreviewUtf8Bytes = previewBytes
  }
  let longPlainTextCharacters = 0
  let longTranscriptUtf8Bytes = 0
  for (const transcript of longTranscripts) {
    const tail = lastSaid(transcript).length
    if (tail > longPlainTextCharacters) longPlainTextCharacters = tail
    const size = transcriptUtf8Bytes(transcript)
    if (size > longTranscriptUtf8Bytes) longTranscriptUtf8Bytes = size
  }
  return {
    generator: "seeded-workspace",
    algorithm: "mulberry32",
    seed: spec.seed,
    now: spec.now,
    sessions: sessions.length,
    channels: channels.length,
    longTranscripts: longTranscripts.length,
    messages: longTranscripts[0]?.messages.length ?? 0,
    maxTitleCharacters,
    maxPreviewUtf8Bytes,
    longPlainTextCharacters,
    longTranscriptUtf8Bytes,
    statusCounts,
    largestChannelSessions: Math.max(0, ...channelCounts.values()),
  }
}

/** Build a workspace for `spec`. The same spec returns the same sessions. */
export function seededWorkspace(
  spec: SeededWorkspaceSpec,
  models: readonly ComposerModel[] = composerModels,
): SeededWorkspace {
  accepted(spec)
  const model = modelOf(models)
  const sessions: SessionSummary[] = []
  const transcripts = new Map<string, Transcript>()
  const longTranscripts: Transcript[] = []
  let cursor: Cursor = { state: spec.seed >>> 0 }
  for (let index = 0; index < spec.sessions; index += 1) {
    const built = sessionAt(index, cursor, spec, model)
    cursor = built.cursor
    const long = index < spec.longTranscripts
    const transcript = transcriptFor(
      built.session,
      built.openingText,
      long ? spec.messages : 1,
      long ? spec.messageCharacters : 0,
    )
    const firstAt = transcript.messages[0]?.at
    const session = {
      ...built.session,
      preview: lastSaid(transcript),
      ...(firstAt !== undefined && firstAt < built.session.startedAt
        ? { startedAt: firstAt }
        : {}),
    }
    sessions.push(session)
    transcripts.set(session.id, transcript)
    if (long) longTranscripts.push(transcript)
  }
  const kept = consistentIndex({
    sections: [...sections],
    channels: [...channels],
    sessions,
  })
  const keptIds = new Set(kept.index.sessions.map((session) => session.id))
  for (const id of [...transcripts.keys()]) if (!keptIds.has(id)) transcripts.delete(id)
  return {
    index: kept.index,
    transcripts,
    contradictions: kept.contradictions,
    report: reportFor(
      spec,
      kept.index.sessions,
      longTranscripts.filter((transcript) => keptIds.has(transcript.sessionId)),
    ),
  }
}
