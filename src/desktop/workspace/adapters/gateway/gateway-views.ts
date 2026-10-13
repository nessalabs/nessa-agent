/**
 * The gateway's conversations read as the workspace's own types: a list row
 * as a session's summary, a conversation view as its transcript, a pending
 * permission as its approval. Pure — no client, no clock: what the adapter
 * (`gateway-source.ts`) knows besides the wire is passed in, and the
 * revisions it mints are its own. A turn the gateway says failed, was
 * cancelled, or was left unresolved keeps that status and the diagnostic it
 * sent. A typed authentication refusal does not also carry them. Questions
 * the view lists, and a notice that a review or question was left out, are
 * kept; a question or notice that names nothing is left out.
 *
 * The gateway has no sections or channels, so the index has one of each and
 * every conversation is in it. It keeps no pin and no unread mark, so every
 * summary is unpinned and read. Its summaries name no model: the one passed
 * in is the adapter's best word for it (`modelFor`).
 */
import {
  clientDueAtMs,
  type ConversationMessageApp,
  type ConversationPermission,
  type ConversationSummary,
  type ConversationTool,
  type ConversationView,
} from "@nessa/client"
import {
  historyExtent,
  placeNotes,
  placePublished,
  shownNote,
} from "../../../../conversation"
import { composerModels } from "../../../model/composer-options"
import { decodeId, encodeId } from "../../../model/id-encoding"
import type {
  Approval,
  ApprovalChoice,
  ApprovalOption,
  ApprovalOrigin,
  Message,
  MessageApp,
  Part,
  StepKind,
  Transcript,
  TranscriptQuestion,
  TurnMessage,
  TurnTerminal,
} from "../../model/transcript"
import {
  defaultModel,
  type Channel,
  type ModelRef,
  type Section,
  type SessionSummary,
} from "../../model/workspace-index"
import { gatewayToolWidget } from "./tool-widget"

/** The one section the gateway's conversations are listed under. */
export const gatewaySection: Section = { id: "gateway", name: "Nessa" }

/** The one channel every gateway conversation is in. */
export const gatewayChannel: Channel = {
  id: "gateway-conversations",
  name: "Conversations",
  sectionId: gatewaySection.id,
  private: false,
  topic: "",
}

/** What the adapter knows of a session besides its list row. */
export interface SummaryContext {
  readonly model: ModelRef
  /** The last conversation read of it asks the person something. */
  readonly waitingOnPerson: boolean
}

/** A list row as the session's summary, without the revision the adapter mints for it. */
export function summaryFrom(
  row: ConversationSummary,
  context: SummaryContext,
): Omit<SessionSummary, "revision"> {
  return {
    id: row.conversationId,
    channelId: gatewayChannel.id,
    title: row.title ?? "",
    model: context.model,
    status: context.waitingOnPerson ? "needs-you" : row.running ? "running" : "idle",
    startedAt: row.createdAtMs,
    updatedAt: row.updatedAtMs,
    preview: row.preview ?? "",
    pinned: false,
    unread: false,
  }
}

type Said = Omit<SessionSummary, "revision">

/**
 * How each field of a summary is compared: a total table, so a field added to
 * `SessionSummary` does not compile until it says how it is compared.
 */
const sameField: {
  readonly [K in keyof Required<Said>]: (a: Said[K], b: Said[K]) => boolean
} = {
  id: Object.is,
  channelId: Object.is,
  title: Object.is,
  model: (a, b) => a.provider === b.provider && a.modelId === b.modelId,
  status: Object.is,
  startedAt: Object.is,
  updatedAt: Object.is,
  preview: Object.is,
  now: Object.is,
  pinned: Object.is,
  unread: Object.is,
}

/** Whether two summaries say the same, revisions aside. */
export function sameSummary(a: Said, b: Said): boolean {
  return (Object.keys(sameField) as (keyof Said)[]).every((key) =>
    (sameField[key] as (x: unknown, y: unknown) => boolean)(a[key], b[key]),
  )
}

/** The catalogue's entry for the model a view says its conversation runs on, if it lists one. */
export function runningModel(view: ConversationView | undefined): ModelRef | undefined {
  const running = view?.runtime?.model
  const listed = composerModels.find((model) => model.modelId === running)
  return listed && { provider: listed.provider, modelId: listed.modelId }
}

/**
 * The model a session's summary names: the one it is known to run on (the
 * adapter's `knownModel`), else, knowing nothing, the composer's default — a
 * guess, said here because the gateway's list names no model.
 */
export function modelFor(known: ModelRef | undefined): ModelRef {
  return known ?? defaultModel() ?? { provider: "", modelId: "" }
}

/**
 * An approval's id: the execution and the permission that name the review,
 * each encoded so the separator cannot occur in either (`id-encoding.ts`).
 */
export function approvalId(permission: ConversationPermission): string {
  return `${encodeId(permission.executionId)}/${encodeId(permission.permissionId)}`
}

/**
 * Who asked for a review, as the workspace says it: the agent, or an MCP App
 * naming the tool it asked to call. A total table, so a kind the protocol
 * adds does not compile until it says who that is. The client refuses a view
 * with an app's review that names no server or tool (`conversation-validate.ts`;
 * its test "reads who asked for a review"), so the empty text below stands in
 * for nothing a read can hold.
 */
/**
 * What a view's option decides, as the card's choice. A total table: an
 * effect the protocol adds does not compile until it says which choice it is.
 * The view has no effect that reaches past this request.
 */
const approvalChoices: {
  readonly [K in ConversationPermission["options"][number]["effect"]]: ApprovalChoice
} = {
  allow: "once",
  deny: "deny",
}

/** The answers a review offers, in the order the view lists them. */
function approvalOptions(permission: ConversationPermission): readonly ApprovalOption[] {
  return permission.options.map((option) => ({
    id: option.id,
    label: option.label,
    choice: approvalChoices[option.effect],
  }))
}

const approvalOrigins: {
  readonly [K in ConversationPermission["origin"]["kind"]]: (
    origin: ConversationPermission["origin"],
  ) => ApprovalOrigin
} = {
  harness: () => ({ kind: "agent" }),
  app: (origin) => ({
    kind: "app",
    server: origin.server ?? "",
    tool: origin.tool ?? "",
  }),
}

/** The review an approval id names, or `null` for an id this adapter did not write. */
export function reviewOf(
  id: string,
): { readonly executionId: string; readonly permissionId: string } | null {
  const halves = id.split("/")
  if (halves.length !== 2) return null
  const executionId = decodeId(halves[0])
  const permissionId = decodeId(halves[1])
  return executionId === null || permissionId === null
    ? null
    : { executionId, permissionId }
}

/**
 * A message's id. A turn's input is under its execution id, encoded — the
 * window's own message ids are UUIDs, which encode as themselves, so the
 * message it sent is found under the id it sent it with — and the agent's
 * reply under its execution id and `/reply`: an encoded id has no `/`, so
 * no reply's id is any input's.
 */
export const inputId = (executionId: string) => encodeId(executionId)
export const replyId = (executionId: string) => `${executionId}/reply`
/**
 * The messages holding files the agent published outside any turn the view
 * shows (`placePublished`): those of turns it left out, before the first turn,
 * and those published while no turn ran, after the last. Each begins with `/`,
 * which neither an input's id (encoded: no `/`) nor a reply's (an execution
 * id, then `/reply`) can.
 */
export const publishedBeforeId = "/published/before"
export const publishedAfterId = "/published"
/**
 * The message a note somebody left is shown as, under the note's own name,
 * encoded: `/note:` and no other `/`, which no input's id (no `/`), reply's
 * (ending `/reply`) or published files' (`/published…`) can be.
 */
export const noteId = (messageId: string) => `/note:${encodeId(messageId)}`

/** A file the agent published, as the part a message shows it by. */
function publishedPart(artifact: ConversationView["artifacts"][number]): Part {
  return {
    kind: "file",
    name: artifact.name,
    digest: artifact.digest,
    mimeType: artifact.mimeType,
    size: artifact.size,
  }
}

/** What a tool call did, in the window's four kinds; the provider's other kinds read as run. */
function stepKind(kind: ConversationTool["kind"]): StepKind {
  switch (kind) {
    case "read":
      return "read"
    case "edit":
    case "delete":
    case "move":
      return "edit"
    case "search":
    case "fetch":
      return "search"
    default:
      return "run"
  }
}

/**
 * The MCP App that wrote a turn of the person's, by its server and tool, as
 * the view names it (#390); nothing for the person's own.
 */
function writtenBy(app: ConversationMessageApp | undefined): { app?: MessageApp } {
  return app ? { app: { server: app.server, tool: app.tool } } : {}
}

/**
 * A conversation view as the session's transcript, at the revision the
 * adapter minted for it. `seen` answers when the adapter first saw a message
 * — the gateway's view carries no message times — and keeps answering the same.
 *
 * Each turn is the input under its execution id and, once it has output,
 * the agent's reply: its text fragments joined where the provider says they
 * belong together, a Nessa notice as text, each tool call a step and — for a
 * tool that declared an MCP App — the widget part its UI is drawn in
 * (`gatewayToolWidget`). A turn the gateway says failed, was cancelled, or
 * was left unresolved keeps that status and, when the gateway sent one, its
 * diagnostic — on the reply when there is one, otherwise on the input.
 * Authentication recovery stays the latest turn's typed refusal, and that
 * turn does not also carry the diagnostic. Thoughts are not shown. A question
 * the agent is waiting on is kept as the view listed it. A notice that a
 * review or question was left out of the view is kept in the view's words. A file the agent published
 * ends the reply of the turn that was running when it was published; one
 * published by a turn the bounded view left out is in a message of its own
 * before the first turn, and one published while none ran in one after the
 * last (`placePublished`). Inputs still
 * waiting follow, so a message the window sent is found as soon as the
 * gateway holds it. A note somebody left is a message of its own right after
 * the turn it followed, or the input still waiting it followed; one left
 * before any turn, or after a turn the bounded view left out, is first
 * (`placeNotes`).
 *
 * The view's `truncated` and `transcriptState` are carried as `history`.
 * This surface does not fold committed records to fill in what the view
 * left out (one fold, `docs/design/record-subscriptions.md`).
 */

export function transcriptFrom(
  view: ConversationView,
  revision: number,
  seen: (messageId: string) => number,
  /** When the view arrived, on the window's clock: its deadline is put on that clock. */
  receivedAtMs: number,
): Transcript {
  const tools = new Map(
    view.tools.map((tool) => [JSON.stringify([tool.executionId, tool.toolId]), tool]),
  )
  const listed = new Set(view.messages.map((turn) => turn.executionId))
  const published = placePublished(view.artifacts, listed)
  // The inputs still waiting that the view lists apart from its turns: drawn
  // after every turn, as the panel draws them.
  const unlisted = view.pending.filter((waiting) => !listed.has(waiting.executionId))
  const notes = placeNotes(
    view.notes,
    listed,
    unlisted.map((waiting) => waiting.executionId),
  )
  const messages: Message[] = []
  const pushNotes = (left: readonly ConversationView["notes"][number][]) => {
    for (const note of left) {
      const id = noteId(note.messageId)
      messages.push({ id, role: "note", at: seen(id), parts: [], note: shownNote(note) })
    }
  }
  pushNotes(notes.before)
  if (published.before.length > 0)
    messages.push({
      id: publishedBeforeId,
      role: "agent",
      at: seen(publishedBeforeId),
      parts: published.before.map(publishedPart),
    })
  let activity: Transcript["activity"] = null
  for (const turn of view.messages) {
    const input = inputId(turn.executionId)
    const user: TurnMessage = {
      id: input,
      role: "user",
      at: seen(input),
      parts: [{ kind: "text", text: turn.userText }],
      ...writtenBy(turn.app),
    }
    const parts: Part[] = []
    // The provider message the last text part came from, while text is last.
    let textOf: string | undefined
    let lastTool: ConversationTool | undefined
    for (const part of turn.parts) {
      if (part.kind === "text" || part.kind === "local_notice") {
        const last = parts[parts.length - 1]
        // Only fragments the provider names as one message are one piece of text.
        if (
          part.kind === "text" &&
          last?.kind === "text" &&
          part.messageId !== undefined &&
          textOf === part.messageId
        )
          parts[parts.length - 1] = { kind: "text", text: last.text + part.text }
        else parts.push({ kind: "text", text: part.text })
        textOf = part.kind === "text" ? part.messageId : undefined
      } else if (part.kind === "tool") {
        textOf = undefined
        const tool = tools.get(JSON.stringify([turn.executionId, part.toolId]))
        // A tool the bounded view left out is not guessed at.
        if (!tool) continue
        lastTool = tool
        parts.push({ kind: "step", step: stepKind(tool.kind), label: tool.title })
        const widget = gatewayToolWidget(view.conversationId, tool)
        if (widget) parts.push(widget)
      }
    }
    parts.push(...(published.byExecution.get(turn.executionId) ?? []).map(publishedPart))
    const terminal = turnTerminal(turn)
    if (parts.length > 0) {
      const reply = replyId(turn.executionId)
      const agent: TurnMessage = { id: reply, role: "agent", at: seen(reply), parts }
      messages.push(user)
      messages.push(terminal ? { ...agent, terminal } : agent)
    } else messages.push(terminal ? { ...user, terminal } : user)
    pushNotes(notes.byExecution.get(turn.executionId) ?? [])
    // A turn at work with nothing streaming yet: what it is doing, and since it was asked.
    if (
      (turn.status === "running" || turn.status === "queued") &&
      parts[parts.length - 1]?.kind !== "text"
    )
      activity = { label: lastTool?.title || "Thinking", since: seen(input) }
  }
  if (published.after.length > 0)
    messages.push({
      id: publishedAfterId,
      role: "agent",
      at: seen(publishedAfterId),
      parts: published.after.map(publishedPart),
    })
  for (const waiting of unlisted) {
    const input = inputId(waiting.executionId)
    messages.push({
      id: input,
      role: "user",
      at: seen(input),
      parts: [{ kind: "text", text: waiting.text }],
      // Waiting, it is labelled as it will be once it is a turn (D-H).
      ...writtenBy(waiting.app),
    })
    pushNotes(notes.byWaiting.get(waiting.executionId) ?? [])
  }
  const latestInputId = unlisted.at(-1)?.executionId ?? view.messages.at(-1)?.executionId
  const latestTurn = view.messages.at(-1)
  const history = historyExtent(view)
  // What runs is the tool and its exact input, which the gateway offers a
  // review only when it can show whole; why is the provider's title for it.
  const asked = view.permissions[0]
  const approval: Approval | null = asked
    ? {
        id: approvalId(asked),
        command: `${asked.toolName} ${asked.argumentsJson}`,
        reason: asked.title,
        origin: approvalOrigins[asked.origin.kind](asked.origin),
        options: approvalOptions(asked),
        // The gateway's own word for what is asked, a closed set the client
        // holds the view to (`conversation-validate.ts`, D19 on #390).
        ask: asked.ask,
        deadline: asked.deadline && {
          waitMs: asked.deadline.waitMs,
          // The gateway's clock's moment as the window's, so the card's
          // countdown does not move with the difference between the two.
          dueAtMs: clientDueAtMs(asked.deadline.dueAtMs, view.serverNowMs, receivedAtMs),
          effect: asked.deadline.effect,
          reason: asked.deadline.reason,
          stopsTurn: asked.deadline.stopsTurn,
        },
      }
    : null
  return {
    sessionId: view.conversationId,
    messages,
    activity,
    approval,
    revision,
    agent: view.runtime?.agent,
    lease: view.lease && {
      state: view.lease.state,
      environment: view.lease.environment,
      host: view.lease.host,
      cause: view.lease.cause,
      refusal: view.lease.refusal,
    },
    authenticationRefusal: latestTurn?.authenticationRequired
      ? latestTurn.executionId
      : undefined,
    questions: questionsFrom(view),
    interactionNotice: reportedText(view.interactionViewError),
    latestInputId,
    ...(history ? { history } : {}),
  }
}

/**
 * A terminal status the gateway reported, apart from a typed authentication
 * refusal, which the recovery card owns. Any other status is still running
 * or finished and has no mark. A blank diagnostic is left off; the text
 * itself is not rewritten.
 */
function turnTerminal(turn: ConversationView["messages"][number]): TurnTerminal | undefined {
  if (turn.authenticationRequired) return undefined
  switch (turn.status) {
    case "failed":
    case "cancelled":
    case "unresolved": {
      const error = reportedText(turn.error)
      return error === undefined ? { status: turn.status } : { status: turn.status, error }
    }
    case "queued":
    case "running":
    case "completed":
    case "injected":
      return undefined
  }
}

/** The gateway's text when it sent some. Blank is the same as absent. */
function reportedText(value: string | undefined): string | undefined {
  if (value === undefined || value.trim() === "") return undefined
  return value
}

/**
 * Questions the view lists that name an ask and at least one field. One that
 * does not is left out; the rest of the conversation is still shown.
 */
function questionsFrom(view: ConversationView): readonly TranscriptQuestion[] {
  const questions: TranscriptQuestion[] = []
  for (const ask of view.questions) {
    if (!ask.executionId || !ask.questionId || !reportedText(ask.message)) continue
    const fields = ask.questions.flatMap((question) => {
      if (!question.key || !reportedText(question.prompt)) return []
      const options = question.options.flatMap((option) => {
        if (!option.value || !reportedText(option.label)) return []
        const description = reportedText(option.description ?? undefined)
        return [
          {
            value: option.value,
            label: option.label,
            ...(description === undefined ? {} : { description }),
          },
        ]
      })
      if (options.length === 0 && !question.freeText) return []
      const header = reportedText(question.header ?? undefined)
      return [
        {
          key: question.key,
          prompt: question.prompt,
          ...(header === undefined ? {} : { header }),
          multiSelect: question.multiSelect,
          freeText: question.freeText,
          required: question.required,
          options,
        },
      ]
    })
    if (fields.length === 0) continue
    questions.push({
      executionId: ask.executionId,
      questionId: ask.questionId,
      message: ask.message,
      questions: fields,
    })
  }
  return questions
}
