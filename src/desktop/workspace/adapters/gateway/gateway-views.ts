/**
 * The gateway's conversations read as the workspace's own types: a list row
 * as a session's summary, a conversation view as its transcript, a pending
 * permission as its approval. Pure — no client, no clock: what the adapter
 * (`gateway-source.ts`) knows besides the wire is passed in, and the
 * revisions it mints are its own. A turn the gateway says failed, was
 * cancelled, or was left unresolved keeps that status and the diagnostic it
 * sent, including a typed authentication refusal. The pane omits that
 * refusal's mark only while the sign-in card is showing. A local retry that
 * hides the card therefore still has the mark. `requiredWorkFailed` stays on
 * the message that carries the mark, so the pane keeps an independent
 * required-work failure beside the card. Questions
 * the view lists, and a notice that a review or question was left out, are
 * kept; a question or notice that names nothing is left out.
 *
 * The gateway has no sections, so the index has one, and every channel the
 * gateway lists is in it: its default channel as `gatewayChannel`, then the
 * person's own (`channelFrom`). It keeps no pin and no unread mark, so every
 * summary is unpinned and read. Its summaries name no model: the one passed
 * in is the adapter's best word for it (`modelFor`).
 */
import {
  clientDueAtMs,
  type ConversationAgentSource,
  type ConversationMessageApp,
  type ImageAttachment,
  type LinkedFile,
  type ConversationPermission,
  type ConversationReviewEntry,
  type QuestionDetail,
  type ReviewDetail,
  type ConversationSummary,
  type ConversationTool,
  type ConversationView,
  type Channel as WireChannel,
} from "@nessa/client"
import {
  referencedContent,
  type AgentSource,
  type UploadedFileReference,
  historyExtent,
  placeNotes,
  placePublished,
  shownNote,
  usageLimitOnClientClock,
} from "../../../../conversation"
import { agentForProvider, composerModels } from "../../../model/composer-options"
import { encodeId } from "../../../model/id-encoding"
import type {
  Approval,
  ApprovalChoice,
  ApprovalOption,
  ApprovalOrigin,
  Message,
  MessageApp,
  Part,
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
import { toolStep } from "./gateway-steps"
import { gatewayToolWidget } from "./tool-widget"

/** The one section the gateway's conversations are listed under. */
export const gatewaySection: Section = { id: "gateway", name: "Nessa" }

/** The gateway's default channel: every conversation made in no other. */
export const gatewayChannel: Channel = {
  id: "gateway-conversations",
  name: "Conversations",
  sectionId: gatewaySection.id,
  private: false,
  topic: "",
}

/** The gateway's id for its default channel, which the index names `gatewayChannel`. */
const defaultChannelId = "default"

/** A channel's id in the index, from the gateway's: its default one is `gatewayChannel`. */
export function channelIdFrom(gatewayId: string | undefined): string {
  return gatewayId === undefined || gatewayId === defaultChannelId
    ? gatewayChannel.id
    : gatewayId
}

/** The gateway's id for a channel the index names, for `conversation.create`; none for its default. */
export function gatewayChannelId(channelId: string): string | undefined {
  return channelId === gatewayChannel.id ? undefined : channelId
}

/** The catalogue's entry for a gateway agent's model, if the catalogue lists it. */
function catalogueModel(agent: string, modelId: string): ModelRef | undefined {
  const listed = composerModels.find(
    (model) => model.modelId === modelId && agentForProvider(model.provider) === agent,
  )
  return listed && { provider: listed.provider, modelId: listed.modelId }
}

/**
 * A channel the gateway lists, as the index holds it. The default channel is
 * `gatewayChannel`, with the folder the gateway says it works in. A channel
 * whose model the catalogue does not list has no defaults: its new sessions
 * start as any other.
 */
export function channelFrom(wire: WireChannel): Channel {
  if (wire.channelId === defaultChannelId)
    return { ...gatewayChannel, folder: wire.folder }
  const model =
    wire.agent && wire.model ? catalogueModel(wire.agent, wire.model) : undefined
  return {
    id: wire.channelId,
    name: wire.name,
    sectionId: gatewaySection.id,
    private: false,
    topic: "",
    folder: wire.folder,
    ...(wire.coordinatorConversationId
      ? { coordinatorConversationId: wire.coordinatorConversationId }
      : {}),
    coordinatorPlacement: wire.coordinatorPlacement,
    threadPlacement: wire.threadPlacement,
    ...(model ? { defaults: { model, access: wire.approvalMode } } : {}),
  }
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
    channelId: channelIdFrom(row.channelId),
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

/**
 * The catalogue's entry for the model a view says its conversation selected,
 * if the catalogue lists it. The selected model is on the view whether or
 * not an agent is open. The runtime's model is not a substitute: a missing
 * selection is not the catalogue default and not whatever the process last
 * reported.
 */
export function runningModel(view: ConversationView | undefined): ModelRef | undefined {
  const running = view?.model
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
 * An approval's id: the review's key as the view lists it, new for every
 * review opened, so a review whose ids are used again is a new approval.
 */
export function approvalId(entry: ConversationReviewEntry): string {
  return entry.review
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

/**
 * The option id of Nessa's project allow rule. The card's always choice is
 * this option. A harness `allow_always` keeps its own id and stays a once
 * choice, so selecting it sends that id back and stores no Nessa rule.
 */
export const REMEMBER_ALLOW_OPTION = "nessa.remember.allow"

/** The answers a review offers, in the order the view lists them. */
function approvalOptions(permission: ConversationPermission): readonly ApprovalOption[] {
  return permission.options.map((option) => ({
    id: option.id,
    label: option.label,
    choice:
      option.id === REMEMBER_ALLOW_OPTION ? "always" : approvalChoices[option.effect],
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

/** The review key an approval id names, or `null` for an id this adapter did not write. */
export function reviewOf(id: string): string | null {
  return /^[0-9a-f]{32}$/.test(id) ? id : null
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
/**
 * The line for a record this read could not apply, under that record's
 * position. `/unreadable:` and no other `/`, which no input, reply, note or
 * published-files id can be.
 */
export const unreadableId = (id: string) => `/unreadable:${encodeId(id)}`

/**
 * The line where the agent ran on in a new session, under the handoff's own
 * name: `/handoff:` and no other `/`. A fork's copied turns are under its
 * name and their place in the copy, `/handoff:<name>/<n>` for what was said
 * and `/handoff:<name>/<n>/reply` for the reply, which no other id can be.
 */
export const handoffId = (id: string) => `/handoff:${encodeId(id)}`
const carriedId = (id: string, index: number, reply: boolean) =>
  `${handoffId(id)}/${index}${reply ? "/reply" : ""}`

/** The final line when the gap list was cut. Not a record position. */
export const unreadableOverflowId = "/unreadable-more"

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

/**
 * The MCP App that wrote a turn of the person's, by its server and tool, as
 * the view names it (#390); nothing for the person's own.
 */
function writtenBy(
  app: ConversationMessageApp | undefined,
  agent: ConversationAgentSource | undefined,
): { app?: MessageApp; agent?: AgentSource } {
  return {
    ...(app ? { app: { server: app.server, tool: app.tool } } : {}),
    ...(agent ? { agent } : {}),
  }
}

/** Input references use the conversation owner's mapping, without reconstructed metadata. */
function inputParts(
  text: string,
  images: readonly ImageAttachment[],
  files: readonly LinkedFile[],
  uploads: readonly UploadedFileReference[] = [],
): Part[] {
  const content = referencedContent("", images, files, uploads).filter(
    (part) =>
      part.type === "image-reference" ||
      part.type === "file-reference" ||
      part.type === "uploaded-file-reference",
  )
  if (!text && content.length === 0) return []
  return [
    { kind: "text", text },
    ...(content.length ? [{ kind: "attachments" as const, content }] : []),
  ]
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
 * Authentication recovery stays the latest input's typed refusal, and that
 * turn does not also carry the diagnostic. An earlier refusal keeps the
 * mark the gateway reported. Thoughts are not shown. A question
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
 * (`placeNotes`). A record this read could not apply is a line of its own
 * in the same places, and so is a switch to another agent. A fork's copied
 * turns come first of all, as the turns they were, then the line saying
 * they were copied.
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
  /** What each review the view lists shows, as read so far (`reviewDetails`). */
  details: readonly ReviewDetail[] = [],
  /** All each question the view lists asks, as read so far (`questionDetails`). */
  questionDetails: readonly QuestionDetail[] = [],
): Transcript {
  const tools = new Map(
    view.tools.map((tool) => [JSON.stringify([tool.executionId, tool.toolId]), tool]),
  )
  const listed = new Set(view.messages.map((turn) => turn.executionId))
  const published = placePublished(view.artifacts, listed)
  // The inputs still waiting that the view lists apart from its turns: drawn
  // after every turn, as the panel draws them.
  const unlisted = view.pending.filter((waiting) => !listed.has(waiting.executionId))
  const waitingIds = unlisted.map((waiting) => waiting.executionId)
  // Recovery owns a typed refusal only while that turn is still the latest
  // input. A later turn or a later waiting input leaves the older mark.
  const latestInputId = unlisted.at(-1)?.executionId ?? view.messages.at(-1)?.executionId
  const notes = placeNotes(view.notes, listed, waitingIds)
  const unreadable = placeNotes(view.unreadable, listed, waitingIds)
  // A switch goes where a note would; a fork is how the conversation began.
  const switches = placeNotes(
    view.handoffs.filter((handoff) => handoff.kind === "switch"),
    listed,
    waitingIds,
  )
  const messages: Message[] = []
  const pushHandoffs = (left: readonly ConversationView["handoffs"][number][]) => {
    for (const handoff of left) {
      const id = handoffId(handoff.handoffId)
      messages.push({
        id,
        role: "handoff",
        at: seen(id),
        parts: [],
        handoff:
          handoff.kind === "switch" && handoff.from && handoff.to
            ? { kind: "switch", from: handoff.from, to: handoff.to }
            : { kind: "fork", omittedTurns: handoff.omittedTurns ?? 0 },
      })
    }
  }
  // A fork's copied turns are drawn as the turns they were, before the line
  // that says they were copied, and before anything the fork said itself.
  for (const fork of view.handoffs.filter((handoff) => handoff.kind === "fork")) {
    ;(fork.turns ?? []).forEach((turn, index) => {
      if (turn.said !== undefined) {
        const id = carriedId(fork.handoffId, index, false)
        messages.push({
          id,
          role: "user",
          at: seen(id),
          parts: [{ kind: "text", text: turn.said }],
          agent: turn.agent,
        })
      }
      if (turn.reply !== undefined) {
        const id = carriedId(fork.handoffId, index, true)
        messages.push({
          id,
          role: "agent",
          at: seen(id),
          parts: [{ kind: "text", text: turn.reply }],
        })
      }
    })
    pushHandoffs([fork])
  }
  const pushNotes = (left: readonly ConversationView["notes"][number][]) => {
    for (const note of left) {
      const id = noteId(note.messageId)
      messages.push({ id, role: "note", at: seen(id), parts: [], note: shownNote(note) })
    }
  }
  const pushUnreadable = (left: readonly ConversationView["unreadable"][number][]) => {
    for (const part of left) {
      const id = unreadableId(part.id)
      messages.push({
        id,
        role: "unreadable",
        at: seen(id),
        parts: [],
        unreadable: {
          id: part.id,
          ...(part.afterExecutionId === undefined
            ? {}
            : { afterExecutionId: part.afterExecutionId }),
        },
      })
    }
  }
  pushUnreadable(unreadable.before)
  pushNotes(notes.before)
  pushHandoffs(switches.before)
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
    const working = turn.status === "running" || turn.status === "queued"
    const user: TurnMessage = {
      id: input,
      role: "user",
      at: seen(input),
      parts: inputParts(turn.userText, turn.attachments, turn.files, turn.uploads),
      ...writtenBy(turn.app, turn.agent),
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
        parts.push(
          toolStep(tool, {
            turnEnded: !working,
            serverNowMs: view.serverNowMs,
            receivedAtMs,
          }),
        )
        const widget = gatewayToolWidget(view.conversationId, tool)
        if (widget) parts.push(widget)
      }
    }
    parts.push(...(published.byExecution.get(turn.executionId) ?? []).map(publishedPart))
    const terminal = turnTerminal(turn)
    const usageLimit =
      turn.usageLimit === undefined
        ? undefined
        : usageLimitOnClientClock(turn.usageLimit, view.serverNowMs, receivedAtMs)
    const marked =
      terminal || usageLimit
        ? {
            ...(terminal ? { terminal } : {}),
            executionId: turn.executionId,
            ...(turn.requiredWorkFailed === true
              ? { requiredWorkFailed: true as const }
              : {}),
            ...(usageLimit ? { usageLimit } : {}),
          }
        : undefined
    if (parts.length > 0) {
      const reply = replyId(turn.executionId)
      const agent: TurnMessage = {
        id: reply,
        role: "agent",
        at: seen(reply),
        parts,
        // The turn's own status says whether its reply is still being written.
        ...(working ? { working: true as const } : {}),
      }
      if (user.parts.length > 0) messages.push(user)
      messages.push(marked ? { ...agent, ...marked } : agent)
    } else if (user.parts.length > 0)
      messages.push(marked ? { ...user, ...marked } : user)
    else if (marked)
      messages.push({
        id: replyId(turn.executionId),
        role: "agent",
        at: seen(input),
        parts: [],
        ...marked,
      })
    pushUnreadable(unreadable.byExecution.get(turn.executionId) ?? [])
    pushNotes(notes.byExecution.get(turn.executionId) ?? [])
    pushHandoffs(switches.byExecution.get(turn.executionId) ?? [])
    // A turn at work with nothing streaming yet: what it is doing, and since it was asked.
    if (working && parts[parts.length - 1]?.kind !== "text")
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
    const parts = inputParts(
      waiting.text,
      waiting.attachments,
      waiting.files,
      waiting.uploads,
    )
    if (parts.length > 0)
      messages.push({
        id: input,
        role: "user",
        at: seen(input),
        parts,
        // Waiting, it is labelled as it will be once it is a turn (D-H).
        ...writtenBy(waiting.app, waiting.agent),
      })
    pushUnreadable(unreadable.byWaiting.get(waiting.executionId) ?? [])
    pushNotes(notes.byWaiting.get(waiting.executionId) ?? [])
    pushHandoffs(switches.byWaiting.get(waiting.executionId) ?? [])
  }
  if (view.unreadableTruncated)
    messages.push({
      id: unreadableOverflowId,
      role: "unreadable-overflow",
      at: seen(unreadableOverflowId),
      parts: [],
    })
  const latestTurn = view.messages.at(-1)
  const history = historyExtent(view)
  // What runs is the tool and its exact input; why is the provider's title
  // for it. The view lists a review by its key; what it shows is read from
  // its live owner, and the card offers no answer until then (`loading`).
  const approvals: Approval[] = view.permissions.map((entry): Approval => {
    const detail = details.find((each) => each.entry.review === entry.review)
    const asked = detail?.state === "loaded" ? detail.shown : undefined
    return !asked
      ? {
          id: approvalId(entry),
          command: entry.toolName,
          loading: detail?.state === "failed" ? "failed" : "reading",
          reason: "",
          origin:
            entry.origin === "app"
              ? { kind: "app", server: "", tool: entry.toolName }
              : { kind: "agent" },
          options: [],
          ask: "tool",
        }
      : {
          id: approvalId(entry),
          command: `${asked.toolName} ${asked.argumentsJson}`,
          reason: asked.title,
          origin: approvalOrigins[asked.origin.kind](asked.origin),
          options: approvalOptions(asked),
          // The gateway's own word for what is asked, a closed set the client
          // holds the review to (`conversation-validate.ts`, D19 on #390).
          ask: asked.ask,
          deadline: asked.deadline && {
            waitMs: asked.deadline.waitMs,
            // The gateway's clock's moment as the window's, so the card's
            // countdown does not move with the difference between the two.
            dueAtMs: clientDueAtMs(
              asked.deadline.dueAtMs,
              view.serverNowMs,
              receivedAtMs,
            ),
            effect: asked.deadline.effect,
            reason: asked.deadline.reason,
            stopsTurn: asked.deadline.stopsTurn,
          },
        }
  })
  return {
    sessionId: view.conversationId,
    messages,
    activity,
    approvals,
    revision,
    agent: view.runtime?.agent,
    lease: view.lease && {
      state: view.lease.state,
      environment: view.lease.environment,
      host: view.lease.host,
      cause: view.lease.cause,
      refusal: view.lease.refusal,
    },
    modelControl: {
      ...(view.modelChange ? { pending: { ...view.modelChange } } : {}),
      ...(view.modelChangeEffect ? { effect: view.modelChangeEffect } : {}),
    },
    access: {
      mode: view.approvalMode,
      modes: view.approvalModes.map((choice) => choice.id),
      ...(view.approvalModeChange
        ? { changing: view.approvalModeChange.requestedMode }
        : {}),
    },
    authenticationRefusal: latestTurn?.authenticationRequired
      ? latestTurn.executionId
      : undefined,
    ...(view.usageLimitAutoContinue === undefined
      ? {}
      : { usageLimitAutoContinue: view.usageLimitAutoContinue }),
    questions: questionsFrom(view, questionDetails),
    interactionNotice: reportedText(view.interactionViewError),
    latestInputId,
    ...(history ? { history } : {}),
  }
}

/**
 * A terminal status the gateway reported. The mark stays on the message,
 * including a typed authentication refusal: the pane hides a pure refusal's
 * mark only while the sign-in card is showing, and a local retry can hide
 * that card without a new gateway view. Any other status is still running
 * or finished and has no mark. A blank diagnostic is left off; the text
 * itself is not rewritten.
 */
function turnTerminal(
  turn: ConversationView["messages"][number],
): TurnTerminal | undefined {
  const error = reportedText(turn.error)
  switch (turn.status) {
    case "failed":
    case "cancelled":
    case "unresolved":
      return error === undefined
        ? { status: turn.status }
        : { status: turn.status, error }
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
 * does not is left out; the rest of the conversation is still shown. The view
 * lists each by its key; all it asks is read from its live owner, and until
 * then the card offers no answer (`loading`).
 */
function questionsFrom(
  view: ConversationView,
  details: readonly QuestionDetail[],
): readonly TranscriptQuestion[] {
  const questions: TranscriptQuestion[] = []
  const read = new Map(details.map((detail) => [detail.entry.question, detail]))
  for (const entry of view.questions) {
    const detail = read.get(entry.question)
    if (detail?.state !== "loaded") {
      questions.push({
        key: entry.question,
        loading: detail?.state === "failed" ? "failed" : "reading",
        executionId: "",
        questionId: "",
        message: reportedText(entry.message) ?? "The agent has a question",
        questions: [],
      })
      continue
    }
    const ask = detail.shown
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
      key: ask.question,
      executionId: ask.executionId,
      questionId: ask.questionId,
      message: ask.message,
      questions: fields,
    })
  }
  return questions
}
