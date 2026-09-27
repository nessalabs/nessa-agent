import type { Conversation, MessageContent, MessagePart, Receipt } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { findConversation, replaceConversation, withDraft } from "../internal"

/** A stored file as it was before it was uploaded: same bytes here, nothing on the gateway. */
export function notUploaded(content: MessageContent): MessageContent {
  return content.map((part) =>
    part.type === "file" && part.upload.status === "stored"
      ? { ...part, upload: { status: "not-started" as const } }
      : part,
  )
}

/**
 * Forget what the gateway held for a conversation's draft, because it no longer
 * holds it.
 *
 * Closing a conversation on the gateway — which is what Stop does — releases
 * every file that conversation was keeping, sent or not. A draft image that says
 * `stored` afterwards names bytes that are gone, and sending it can only be
 * refused. Putting it back to `not-started` is the honest state, and it is also
 * the fix: the panel uploads whatever is not started. Uploads still in flight
 * are left alone; they settle against the reopened conversation.
 */
export function forgetStoredUploads(tabs: LocalTabs, conversationId: string): LocalTabs {
  const current = findConversation(tabs, conversationId)
  if (!current) return tabs
  const draft = notUploaded(current.draft)
  if (draft.every((part, index) => part === current.draft[index])) return tabs
  return replaceConversation(tabs, withDraft(current, draft))
}

// The gateway has the message, so this window will never need to put its files
// back in a draft. A turn still sending, of unknown delivery, or refused may.
const settled: readonly Receipt[] = ["accepted", "queued", "delivered"]

function sentFile(part: MessagePart) {
  return part.type === "file" && part.upload.status === "stored" ? part : undefined
}

/**
 * Keep at most `maxBytes` of already-sent originals for the transcript to paint,
 * oldest first to go.
 *
 * A sent turn shows its images from the original files this window still holds,
 * and an original can be a 60 MiB RAW. Left alone, a handful of sent messages
 * would hold the whole window's attachment budget while the draft sits empty.
 * So once the gateway has a message, its originals are only a preview: past the
 * budget the oldest become the same labelled reference tile a reloaded turn
 * gets, and their bytes are released. Tabs in order, turns in order — there is
 * no clock in here, and "oldest" needs none to be the same answer every time.
 */
export function boundSentPreviews(tabs: LocalTabs, maxBytes: number): LocalTabs {
  let held = 0
  for (const conversation of tabs.conversations)
    for (const turn of conversation.turns)
      if (turn.from === "user" && settled.includes(turn.receipt))
        for (const part of turn.content) held += sentFile(part)?.size ?? 0
  if (held <= maxBytes) return tabs
  const conversations = tabs.conversations.map((conversation): Conversation => {
    if (held <= maxBytes) return conversation
    return {
      ...conversation,
      turns: conversation.turns.map((turn) => {
        if (turn.from !== "user" || !settled.includes(turn.receipt)) return turn
        return {
          ...turn,
          content: turn.content.map((part) => {
            const file = sentFile(part)
            if (!file || file.upload.status !== "stored" || held <= maxBytes) return part
            held -= file.size
            return { type: "image-reference" as const, ...file.upload.image }
          }),
        }
      }),
    }
  })
  return { ...tabs, conversations }
}
