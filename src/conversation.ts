/**
 * The conversation strip the panel shows. There is no agent behind this yet —
 * `draftReply` is a stand-in so the bubbles, the typing dots, the streaming
 * reveal, and the composer's lit rim are all driven by the same shape a
 * runtime will later drive.
 *
 * These helpers are the rules the surface already has: a conversation is named
 * after its opening line, drafts belong to the conversation, the strip never
 * empties. They live here rather than in the React tree so the panel chrome
 * can render without owning them.
 */

/** How long the typing dots hold before the reply starts arriving. */
export const THINKING_MS = 800
/** How long a reply streams before it settles into a plain bubble. */
export const STREAMING_MS = 2200
/** A conversation is named after its opening line, cut to this. */
const TITLE_LENGTH = 24

export interface Turn {
  id: string
  from: "user" | "assistant"
  text: string
}

/** `thinking` shows the typing dots; `streaming` reveals the reply. */
export type Phase = "idle" | "thinking" | "streaming"

export interface Conversation {
  id: string
  title: string
  turns: Turn[]
  phase: Phase
  /** The prompt awaiting a reply, held while the dots are up. */
  pending: string
  /** Drafts belong to a conversation, not to the composer: switching tabs
   *  mid-sentence must not carry the sentence into someone else's thread. */
  draft: string
}

export function conversation(id: string): Conversation {
  return { id, title: "New chat", turns: [], phase: "idle", pending: "", draft: "" }
}

/**
 * Stands in for the agent runtime that has not been wired up yet.
 */
export function draftReply(prompt: string) {
  return `You said "${prompt}". There is no agent behind this window yet — this is Nessa's chat UI and pill composer running in a floating Tauri panel.`
}

export function titleFor(prompt: string) {
  const trimmed = prompt.trim()
  return trimmed.length > TITLE_LENGTH
    ? `${trimmed.slice(0, TITLE_LENGTH).trimEnd()}…`
    : trimmed
}

/** Records a sent prompt. Returns the same conversation when there is nothing to send. */
export function send(
  current: Conversation,
  prompt: string,
  turnId: string,
): Conversation {
  const trimmed = prompt.trim()
  if (trimmed === "" || current.phase !== "idle") return current
  return {
    ...current,
    title: current.turns.length === 0 ? titleFor(trimmed) : current.title,
    turns: [...current.turns, { id: turnId, from: "user", text: trimmed }],
    pending: trimmed,
    phase: "thinking",
    draft: "",
  }
}

/** Moves a conversation from thinking → streaming, or streaming → idle. */
export function advance(current: Conversation, turnId: string): Conversation {
  if (current.phase === "thinking") {
    return {
      ...current,
      phase: "streaming",
      turns: [
        ...current.turns,
        { id: turnId, from: "assistant", text: draftReply(current.pending) },
      ],
    }
  }
  if (current.phase === "streaming") {
    return { ...current, phase: "idle" }
  }
  return current
}

/** Stops a reply in flight. Idle conversations are unchanged. */
export function stop(current: Conversation): Conversation {
  if (current.phase === "idle") return current
  return { ...current, phase: "idle" }
}

/**
 * Removes a conversation from the strip. The strip is the app's only
 * navigation, so it never empties: closing the last tab replaces it.
 */
export function closeInStrip(
  items: Conversation[],
  id: string,
  activeId: string,
  nextId: () => string,
): { items: Conversation[]; activeId: string } {
  if (items.length === 1) {
    const next = conversation(nextId())
    return { items: [next], activeId: next.id }
  }
  const remaining = items.filter((item) => item.id !== id)
  if (id !== activeId) return { items: remaining, activeId }
  const closed = items.findIndex((item) => item.id === id)
  const next = remaining[closed] ?? remaining[remaining.length - 1]!
  return { items: remaining, activeId: next.id }
}
