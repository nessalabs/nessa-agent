/**
 * The gateway's conversations read as the workspace's own types: a list row
 * as a session's summary, a conversation view as its transcript, a pending
 * permission as its approval. Pure — no client, no clock: what the adapter
 * (`gateway-source.ts`) knows besides the wire is passed in, and the
 * revisions it mints are its own.
 *
 * The gateway has no sections or channels, so the index has one of each and
 * every conversation is in it. It keeps no pin and no unread mark, so every
 * summary is unpinned and read. Its summaries name no model: the one passed
 * in is the adapter's best word for it (`modelFor`).
 */
import type {
  ConversationMessageApp,
  ConversationPermission,
  ConversationSummary,
  ConversationTool,
  ConversationView,
} from "@nessa/client"
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
  UnreadableRow,
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
 * — the gateway's view carries no times — and keeps answering the same.
 *
 * Each turn is the input under its execution id and, once it has output,
 * the agent's reply: its text fragments joined where the provider says they
 * belong together, a Nessa notice as text, each tool call a step and — for a
 * tool that declared an MCP App — the widget part its UI is drawn in
 * (`gatewayToolWidget`). Thoughts are not shown. Inputs still waiting follow,
 * so a message the window sent is found as soon as the gateway holds it.
 */
export function transcriptFrom(
  view: ConversationView,
  revision: number,
  seen: (messageId: string) => number,
): Transcript {
  const tools = new Map(
    view.tools.map((tool) => [JSON.stringify([tool.executionId, tool.toolId]), tool]),
  )
  const messages: Message[] = []
  const anchors: string[] = []
  let activity: Transcript["activity"] = null
  for (const turn of view.messages) {
    const input = inputId(turn.executionId)
    messages.push({
      id: input,
      role: "user",
      at: seen(input),
      parts: [{ kind: "text", text: turn.userText }],
      ...writtenBy(turn.app),
    })
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
    if (parts.length > 0) {
      const reply = replyId(turn.executionId)
      messages.push({ id: reply, role: "agent", at: seen(reply), parts })
      anchors.push(reply)
    } else {
      anchors.push(input)
    }
    // A turn at work with nothing streaming yet: what it is doing, and since it was asked.
    if (
      (turn.status === "running" || turn.status === "queued") &&
      parts[parts.length - 1]?.kind !== "text"
    )
      activity = { label: lastTool?.title || "Thinking", since: seen(input) }
  }
  let latestInputId = view.messages.at(-1)?.executionId
  const listed = new Set(view.messages.map((turn) => turn.executionId))
  for (const waiting of view.pending) {
    if (listed.has(waiting.executionId)) continue
    latestInputId = waiting.executionId
    const input = inputId(waiting.executionId)
    messages.push({
      id: input,
      role: "user",
      at: seen(input),
      parts: [{ kind: "text", text: waiting.text }],
      // Waiting, it is labelled as it will be once it is a turn (D-H).
      ...writtenBy(waiting.app),
    })
  }
  const latestTurn = view.messages.at(-1)
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
      }
    : null
  const unreadable: UnreadableRow[] = (view.unreadable ?? []).map((part) => {
    const index = Math.min(part.afterMessage, anchors.length)
    const afterId = index === 0 ? undefined : anchors[index - 1]
    return {
      sessionId: part.session,
      position: part.position,
      reason: part.reason,
      ...(part.found === undefined ? {} : { found: part.found }),
      ...(afterId === undefined ? {} : { afterId }),
    }
  })
  return {
    sessionId: view.conversationId,
    messages,
    activity,
    approval,
    ...(unreadable.length === 0 ? {} : { unreadable }),
    revision,
    agent: view.runtime?.agent,
    authenticationRefusal: latestTurn?.authenticationRequired
      ? latestTurn.executionId
      : undefined,
    latestInputId,
  }
}
