import type { AttachmentResource, Conversation, MessageContent } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { attachmentResourceUsage } from "../queries/attachment-resources"
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

/** Evict whole preview-only resources; live drafts and unsettled turns retain their bytes. */
export function boundSentPreviews(tabs: LocalTabs, maxBytes: number): LocalTabs {
  const usage = attachmentResourceUsage(tabs)
  let held = [...usage.previews].reduce((sum, resource) => sum + resource.size, 0)
  if (held <= maxBytes) return tabs
  const evicted = new Set<AttachmentResource>()
  for (const resource of usage.previews) {
    if (held <= maxBytes) break
    evicted.add(resource)
    held -= resource.size
  }
  const conversations = tabs.conversations.map((conversation): Conversation => ({
    ...conversation,
    turns: conversation.turns.map((turn) => {
      if (turn.from !== "user") return turn
      return {
        ...turn,
        content: turn.content.map((part) =>
          part.type === "file" &&
          part.upload.status === "stored" &&
          evicted.has(part.resource)
            ? { type: "image-reference" as const, ...part.upload.image }
            : part,
        ),
      }
    }),
  }))
  return { ...tabs, conversations }
}
