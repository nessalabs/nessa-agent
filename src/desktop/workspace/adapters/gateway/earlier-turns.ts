/**
 * The turns a conversation's view lists before its first one, read a page at
 * a time as the person scrolls back (`conversation.readEarlier`), and how
 * they join the view.
 *
 * A view lists the newest turns and says `earlier` when more came before;
 * its first turn can start partway (`startsAt`), the person's message and
 * the turn's older parts read as earlier too. Each page ends exactly where
 * the one after it starts, so pieces of one turn join at their seam: a part
 * cut partway is one part again. What was read is kept per conversation
 * until the conversation is let go, and joined to every view that follows.
 *
 * The view moves on as turns arrive, and its first point with it. What was
 * read then no longer ends where the view starts: the gap between them is
 * read with `after` (`gapOf`), or, when the view now starts inside what was
 * read, what was read is cut back to that turn's start first.
 */
import type {
  ConversationEarlier,
  ConversationMessage,
  ConversationPoint,
  ConversationTool,
  ConversationView,
} from "@nessa/client"

type Handoff = ConversationView["handoffs"][number]

/** What was read before a view's first turn, oldest first. */
export interface EarlierTurns {
  readonly messages: readonly ConversationMessage[]
  readonly tools: readonly ConversationTool[]
  /** The fork the conversation began as, once a page reached the start. */
  readonly forks?: readonly Handoff[]
  /** Something comes before the first message. */
  readonly more: boolean
  /** A page left out something no read gives back. */
  readonly truncated: boolean
  /** The stretch between these and the view could not be read, yet. */
  readonly lost?: boolean
  /** Where these end: the view's first point they were read before. */
  readonly end: ConversationPoint
}

/** Where `message` starts: inside its turn when it starts partway. */
export function startOf(message: ConversationMessage): ConversationPoint {
  return message.startsAt === undefined
    ? { executionId: message.executionId }
    : { executionId: message.executionId, at: message.startsAt }
}

/** Where `point` falls in its turn, comparable within that turn. */
const placeOf = (point: ConversationPoint) =>
  point.at === undefined ? -1 : point.at.part * 2 ** 32 + point.at.byte

export function samePoint(a: ConversationPoint, b: ConversationPoint): boolean {
  return (
    a.executionId === b.executionId &&
    a.at?.part === b.at?.part &&
    a.at?.byte === b.at?.byte
  )
}

/**
 * Pieces of turns, oldest first: `newer` after `older`, a turn split between
 * them one message again. The newer piece starts where the older ends, so a
 * part it cut partway joins the older piece's last part. The joined turn
 * says what the newer piece says of it now — its status, its error — and
 * keeps the person's message and start from the older.
 */
export function joined(
  older: readonly ConversationMessage[],
  newer: readonly ConversationMessage[],
): ConversationMessage[] {
  const last = older.at(-1)
  const first = newer[0]
  if (!last || !first || last.executionId !== first.executionId)
    return [...older, ...newer]
  // Not a piece of it: the same turn whole again, which says it all.
  if (first.startsAt === undefined) return [...older.slice(0, -1), ...newer]
  const parts = [...last.parts]
  let rest = first.parts
  const split = parts.at(-1)
  if (first.startsAt.byte > 0 && split && rest[0]) {
    parts[parts.length - 1] = { ...split, text: split.text + rest[0].text }
    rest = rest.slice(1)
  }
  const { startsAt: _cut, ...current } = first
  const whole: ConversationMessage = {
    ...current,
    parts: [...parts, ...rest],
    userText: last.userText,
    attachments: last.attachments,
    files: last.files,
    ...(last.uploads === undefined ? {} : { uploads: last.uploads }),
    ...(last.app === undefined ? {} : { app: last.app }),
    ...(last.startsAt === undefined ? {} : { startsAt: last.startsAt }),
  }
  return [...older.slice(0, -1), whole, ...newer.slice(1)]
}

const toolKey = (tool: ConversationTool) =>
  JSON.stringify([tool.executionId, tool.toolId])

/** `older` then `newer`, a call listed in both as `newer` says it is now. */
function joinedTools(
  older: readonly ConversationTool[],
  newer: readonly ConversationTool[],
): ConversationTool[] {
  const current = new Set(newer.map(toolKey))
  return [...older.filter((tool) => !current.has(toolKey(tool))), ...newer]
}

/** `page`, read before what `held` holds (or before `end`), put in front of it. */
export function withPage(
  held: EarlierTurns | undefined,
  page: ConversationEarlier,
  end: ConversationPoint,
): EarlierTurns {
  return {
    messages: joined(page.messages, held?.messages ?? []),
    tools: joinedTools(page.tools, held?.tools ?? []),
    ...(page.more ? {} : { forks: page.handoffs }),
    more: page.more,
    truncated: page.truncated || held?.truncated === true,
    end: held?.end ?? end,
  }
}

/** `pages`, read oldest first between `held`'s end and `end`, put after it. */
export function withGap(
  held: EarlierTurns,
  pages: readonly {
    readonly messages: readonly ConversationMessage[]
    readonly tools: readonly ConversationTool[]
    readonly truncated: boolean
  }[],
  end: ConversationPoint,
): EarlierTurns {
  return {
    ...held,
    messages: pages.reduce<ConversationMessage[]>(
      (messages, page) => joined(messages, page.messages),
      [...held.messages],
    ),
    tools: pages.reduce<ConversationTool[]>(
      (tools, page) => joinedTools(tools, page.tools),
      [...held.tools],
    ),
    truncated: held.truncated || pages.some((page) => page.truncated),
    lost: false,
    end,
  }
}

/**
 * Where `held` and `view` no longer meet, and what to read to join them:
 * what lies after `after` and before `before`. When the view now starts
 * inside what was read, `held` is cut back to that turn's start first.
 * Nothing when they meet, or when the view lists no turn to meet.
 */
export function gapOf(
  view: ConversationView,
  held: EarlierTurns,
):
  | {
      readonly held: EarlierTurns
      readonly after: ConversationPoint
      readonly before: ConversationPoint
    }
  | undefined {
  const first = view.messages[0]
  if (!first) return undefined
  const before = startOf(first)
  if (samePoint(held.end, before)) return undefined
  // Still inside the turn what was read ends in, further on: only the
  // stretch between is missing.
  if (held.end.executionId === before.executionId && placeOf(held.end) < placeOf(before))
    return { held, after: held.end, before }
  const inside = held.messages.findIndex(
    (message) => message.executionId === before.executionId,
  )
  if (inside < 0) return { held, after: held.end, before }
  const messages = held.messages.slice(0, inside)
  const kept = new Set(messages.map((message) => message.executionId))
  const cut: EarlierTurns = {
    ...held,
    messages,
    tools: held.tools.filter((tool) => kept.has(tool.executionId)),
    end: { executionId: before.executionId },
  }
  return { held: cut, after: cut.end, before }
}

const utf8 = new TextEncoder()
const fromUtf8 = new TextDecoder()

/**
 * What `view` drew before `before`, a point in it: the turns before that
 * turn whole, and that turn up to the point. Undefined when `before` is not
 * a place `view` drew, or `view` left something out on the way to it.
 */
export function leading(
  view: ConversationView,
  before: ConversationPoint,
): Pick<EarlierTurns, "messages" | "tools"> | undefined {
  if (view.truncated) return undefined
  const at = view.messages.findIndex(
    (message) => message.executionId === before.executionId,
  )
  const turn = view.messages[at]
  if (!turn) return undefined
  const whole = view.messages.slice(0, at)
  const kept = new Set(whole.map((message) => message.executionId))
  const tools = view.tools.filter((tool) => kept.has(tool.executionId))
  if (!before.at) return { messages: whole, tools }
  // Parts and bytes count from the turn's start; this piece starts at its own.
  const from = turn.startsAt ?? { part: 0, byte: 0 }
  const part = before.at.part - from.part
  const byte = before.at.byte - (part === 0 ? from.byte : 0)
  if (part < 0 || byte < 0 || part > turn.parts.length) return undefined
  const parts = turn.parts.slice(0, part)
  if (byte > 0) {
    const split = turn.parts[part]
    const bytes = split && utf8.encode(split.text)
    if (!split || !bytes || byte > bytes.length) return undefined
    parts.push({ ...split, text: fromUtf8.decode(bytes.subarray(0, byte)) })
  }
  const toolIds = new Set(
    parts.filter((each) => each.kind === "tool").map((each) => each.toolId),
  )
  // A view of one turn may have cut its calls' rows to fit (their output,
  // their changes): those are read whole instead.
  if (view.messages.length === 1 && toolIds.size > 0) return undefined
  return {
    messages: [...whole, { ...turn, parts }],
    tools: [
      ...tools,
      ...view.tools.filter(
        (tool) => tool.executionId === turn.executionId && toolIds.has(tool.toolId),
      ),
    ],
  }
}

/**
 * `view` with what was read before it: its turns and calls in front, its
 * fork once a page reached the start, and `earlier` while more comes before.
 * Until then a fork is left out: its copied turns come with the earliest page,
 * and drawn alone it would read as if nothing came between it and the turns
 * listed. A view that lists everything is itself.
 */
export function withEarlier(
  view: ConversationView,
  held: EarlierTurns | undefined,
): ConversationView {
  if (!view.earlier) return view
  const handoffs = view.handoffs.filter((handoff) => handoff.kind !== "fork")
  if (!held) return { ...view, handoffs }
  return {
    ...view,
    messages: joined(held.messages, view.messages),
    tools: joinedTools(held.tools, view.tools),
    handoffs: [...(held.forks ?? []), ...handoffs],
    earlier: held.more,
    truncated: view.truncated || held.truncated || held.lost === true,
  }
}
