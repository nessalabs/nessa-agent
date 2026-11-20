import { useSyncExternalStore } from "react"
import {
  stepApproval,
  type AccessApproval,
  type ApprovalEvent,
} from "../model/access-approval"

/**
 * Each chat's Approve on its access request, kept for the window rather than
 * for the card: a card that leaves the screen (a tab switch, a pane closed)
 * and comes back finds the same record, and a raise that lands while no card
 * is drawn still moves it. One record per chat, keyed by the host's own id for
 * it; `stepApproval` is the only thing that moves one.
 */
const records = new Map<string, AccessApproval<{ readonly askId: string }>>()
/** Chats whose answer is on its way now, so it goes once however many draw it. */
const answering = new Set<string>()
const listeners = new Set<() => void>()

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Moves `chat`'s record by `event`, and tells every card drawing it. */
export function stepChatApproval<Ask extends { readonly askId: string }>(
  chat: string,
  event: ApprovalEvent<Ask>,
): void {
  const now = records.get(chat) as AccessApproval<Ask> | undefined
  const next = stepApproval<Ask>(now, event)
  if (next === now) return
  if (next) records.set(chat, next)
  else records.delete(chat)
  listeners.forEach((listener) => listener())
}

/** `chat`'s record, as drawn now. */
export function useChatApproval<Ask extends { readonly askId: string }>(
  chat: string,
): AccessApproval<Ask> | undefined {
  return useSyncExternalStore(
    subscribe,
    () => records.get(chat) as AccessApproval<Ask> | undefined,
  )
}

/**
 * Sends `chat`'s answer with `answer` if its record is due and no answer is
 * already on its way: the record moves to answering first, so the ask
 * leaving the list while the answer lands is read as that answer.
 */
export function sendChatAnswer(chat: string, answer: (text: string) => Promise<boolean>) {
  const record = records.get(chat)
  if (record?.state !== "due" || answering.has(chat)) return
  answering.add(chat)
  stepChatApproval(chat, { kind: "answering" })
  const label = (record.ask as { choices?: readonly { label: string }[] }).choices?.[0]
    ?.label
  const done = (sent: boolean) => {
    answering.delete(chat)
    stepChatApproval(chat, { kind: "answered", sent })
  }
  if (!label) return done(false)
  answer(label).then(done, () => done(false))
}

/** Forgets every record: for tests, which share one window. */
export function forgetChatApprovals(): void {
  records.clear()
  answering.clear()
  listeners.forEach((listener) => listener())
}
