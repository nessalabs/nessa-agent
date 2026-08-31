/**
 * The language the panel and the future server share. These types are the
 * contract, not the rules: nothing here decides when a send is legal or
 * what a reply contains.
 */

export type Receipt = "sending" | "delivered"

export interface Turn {
  id: string
  from: "user" | "assistant"
  text: string
  receipt?: Receipt
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
