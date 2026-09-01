import { type Conversation, type Turn } from "../../model"
import { titleFor } from "./title"

function deliver(turns: Turn[]): Turn[] {
  return turns.map((turn) =>
    turn.from === "user" && turn.receipt === "sending"
      ? { ...turn, receipt: "delivered" }
      : turn,
  )
}

function emptyAssistant(turns: Turn[]) {
  return [...turns]
    .reverse()
    .find((turn) => turn.from === "assistant" && turn.text === "")
}

function toIdle(current: Conversation, turns: Turn[]): Conversation {
  return {
    id: current.id,
    title: current.title,
    turns,
    draft: current.draft,
    phase: "idle",
  }
}

export function send(
  current: Conversation,
  prompt: string,
  userTurnId: string,
  assistantTurnId: string,
): Conversation {
  const trimmed = prompt.trim()
  if (trimmed === "" || current.phase !== "idle") return current
  return {
    id: current.id,
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

export function advance(
  current: Conversation,
  reply: (prompt: string) => string,
): Conversation {
  if (current.phase === "thinking") {
    const placeholder = emptyAssistant(current.turns)
    if (!placeholder) return current
    return {
      ...current,
      phase: "streaming",
      turns: current.turns.map((turn) =>
        turn.id === placeholder.id ? { ...turn, text: reply(current.pending) } : turn,
      ),
    }
  }
  if (current.phase === "streaming") {
    return toIdle(current, deliver(current.turns))
  }
  return current
}

export function stop(current: Conversation): Conversation {
  if (current.phase === "idle") return current
  return toIdle(
    current,
    deliver(
      current.turns.filter((turn) => !(turn.from === "assistant" && turn.text === "")),
    ),
  )
}

export function withDraft(current: Conversation, draft: string): Conversation {
  return { ...current, draft }
}
