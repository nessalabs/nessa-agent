import { type Conversation, type Turn } from "../../model"
import { titleFor } from "./title"

function deliver(turns: Turn[]): Turn[] {
  return turns.map((turn) =>
    turn.receipt === "sending" ? { ...turn, receipt: "delivered" } : turn,
  )
}

function emptyAssistant(turns: Turn[]) {
  return [...turns]
    .reverse()
    .find((turn) => turn.from === "assistant" && turn.text === "")
}

/**
 * Records a sent prompt and opens the assistant row the thinking chrome
 * will occupy. Same row later holds the reply, so the view does not invent
 * a second identity when tokens arrive. Returns the same conversation
 * when there is nothing to send.
 */
export function send(
  current: Conversation,
  prompt: string,
  userTurnId: string,
  assistantTurnId: string,
): Conversation {
  const trimmed = prompt.trim()
  if (trimmed === "" || current.phase !== "idle") return current
  return {
    ...current,
    title: current.turns.length === 0 ? titleFor(trimmed) : current.title,
    turns: [
      ...current.turns,
      { id: userTurnId, from: "user", text: trimmed, receipt: "sending" },
      { id: assistantTurnId, from: "assistant", text: "" },
    ],
    pending: trimmed,
    phase: "thinking",
    draft: "",
  }
}

/** Fills the open assistant row, or returns the conversation to idle. */
export function advance(
  current: Conversation,
  turnId: string,
  reply: (prompt: string) => string,
): Conversation {
  if (current.phase === "thinking") {
    const placeholder = emptyAssistant(current.turns)
    return {
      ...current,
      phase: "streaming",
      turns: placeholder
        ? current.turns.map((turn) =>
            turn.id === placeholder.id ? { ...turn, text: reply(current.pending) } : turn,
          )
        : [
            ...current.turns,
            { id: turnId, from: "assistant", text: reply(current.pending) },
          ],
    }
  }
  if (current.phase === "streaming") {
    return { ...current, phase: "idle", pending: "", turns: deliver(current.turns) }
  }
  return current
}

/** Stops a reply in flight. Drops an unfilled assistant row. Idle is unchanged. */
export function stop(current: Conversation): Conversation {
  if (current.phase === "idle") return current
  return {
    ...current,
    phase: "idle",
    pending: "",
    turns: deliver(
      current.turns.filter((turn) => !(turn.from === "assistant" && turn.text === "")),
    ),
  }
}

export function withDraft(current: Conversation, draft: string): Conversation {
  return { ...current, draft }
}
