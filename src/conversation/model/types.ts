export type Receipt = "sending" | "delivered"

export type UserTurn = {
  id: string
  from: "user"
  text: string
  receipt: Receipt
}

export type AssistantTurn = {
  id: string
  from: "assistant"
  text: string
}

export type Turn = UserTurn | AssistantTurn

export type IdleConversation = {
  id: string
  title: string
  turns: Turn[]
  draft: string
  phase: "idle"
}

export type BusyConversation = {
  id: string
  title: string
  turns: Turn[]
  draft: string
  phase: "thinking" | "streaming"
  pending: string
}

export type Conversation = IdleConversation | BusyConversation

export type Phase = Conversation["phase"]

export function conversation(id: string): IdleConversation {
  return { id, title: "New chat", turns: [], phase: "idle", draft: "" }
}
