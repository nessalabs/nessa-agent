import {
  contentText,
  messageImages,
  messageLabel,
  MAX_SEND_IMAGES,
  MAX_SEND_TOTAL_IMAGE_BYTES,
  type ImageRefusal,
  type MessageContent,
  type UserTurn,
} from "../../model"
import { findConversation, replaceConversation, takeTurnId } from "../internal/ids"
import type { SubmissionRefusal } from "../ports"
import { notUploaded } from "./release-uploads"
import type { LocalTabs } from "../local-tabs"

/**
 * Retain each local submission independently; busy work does not reject a queueable draft.
 *
 * `content` is the whole message: its text and its files. It becomes a turn only
 * if every file in it can go as an image reference, so a turn never shows a file
 * that was not sent. A message of images alone is a message.
 */
export function beginSend(
  tabs: LocalTabs,
  input: {
    content: MessageContent
    conversationId: string
    executionId: string
    actionId: string
    mode: "queued" | "steering"
  },
): LocalTabs {
  const conv = findConversation(tabs, input.conversationId)
  if (!conv) return tabs
  // Beginning a send empties the draft. A file the draft holds and the message
  // does not would vanish without having gone anywhere, so that is no send.
  const sent = new Set(
    input.content.flatMap((part) => (part.type === "file" ? [part.id] : [])),
  )
  if (conv.draft.some((part) => part.type === "file" && !sent.has(part.id))) return tabs
  const sendable = messageImages(input.content)
  if (!sendable.ok) return tabs
  const text = contentText(input.content)
  if (!text.trim() && sendable.images.length === 0) return tabs
  const firstImage = input.content.find((part) => part.type === "file")
  const taken = takeTurnId(tabs)
  const userTurn: UserTurn = {
    id: taken.id,
    from: "user",
    content: input.content,
    receipt: "sending",
    executionId: input.executionId,
    actionId: input.actionId,
    mode: input.mode,
  }
  return replaceConversation(taken.tabs, {
    ...conv,
    cancellationStatus: undefined,
    title:
      conv.turns.length === 0 && !conv.titleEdited
        ? // A message of images alone is titled by what was attached.
          messageLabel(text, sendable.images.length, firstImage?.name).slice(0, 48)
        : conv.title,
    turns: [...conv.turns, userTurn],
    draft: [],
    phase: "thinking",
    pending: "",
    error: undefined,
  })
}

/** What to tell somebody whose files kept a draft from sending. The draft is always kept. */
export function imageRefusalMessage(refusal: ImageRefusal): string {
  switch (refusal.kind) {
    case "unsupported-file":
      return `"${refusal.name}" cannot be sent: messages carry images, and no other files yet. Remove it to send.`
    case "upload-failed":
      return `"${refusal.name}" did not upload; its tile says why. Retry it there or remove it, then send again.`
    case "upload-in-flight":
      return "Images are still uploading. Send again once they finish."
    case "too-many-images":
      return `A message carries up to ${MAX_SEND_IMAGES} images. Remove some to send.`
    case "images-too-large":
      return `A message carries up to ${MAX_SEND_TOTAL_IMAGE_BYTES / (1024 * 1024)} MiB of images in total, as the gateway stores them. Remove some to send.`
  }
}

/**
 * What to tell somebody whose message the gateway refused before taking it.
 * One sentence per refusal, chosen by its typed reason. `agent-not-configured`
 * and `invalid-request` answer undefined: the client's own message for the
 * first names the remedy, and the second has nothing better to say than it did.
 */
export function submissionRefusalMessage(reason: SubmissionRefusal): string | undefined {
  switch (reason) {
    case "image-input-unsupported":
      return "This agent does not take images, so the message was not sent. It is back in the draft: remove the images to send it."
    case "attachment-not-found":
      return "The gateway no longer holds this message's images — stopping a conversation releases them. The message is back in the draft and its images are uploading again; send once they finish."
    case "attachment-unavailable":
      return "The gateway could not read this message's images, so it was not sent. It is back in the draft and its images are uploading again; send once they finish."
    case "conversation-not-found":
      return "The gateway no longer has this conversation, so the message was not sent. It is back in the draft."
    case "conversation-capacity":
      return "The gateway has too many conversations open to take this one. The message is back in the draft; close a conversation or try again shortly."
    case "agent-not-configured":
    case "invalid-request":
      return undefined
  }
}

/** Whether a refusal means the message's stored images are gone and must be uploaded again. */
export function refusalReleasesImages(reason: SubmissionRefusal): boolean {
  return reason === "attachment-not-found" || reason === "attachment-unavailable"
}

/**
 * A transport failure is not proof that admission failed; retain IDs for explicit retry.
 *
 * `reupload` is for a refusal that says the turn's stored images no longer
 * exist on the gateway. The recovered draft then holds them as not started, so
 * the same dead reference is not offered again and the panel uploads them anew.
 */
export function failSend(
  tabs: LocalTabs,
  conversationId: string,
  executionId: string,
  detail: string,
  uncertain = true,
  reupload = false,
): LocalTabs {
  const conv = findConversation(tabs, conversationId)
  if (!conv) return tabs
  const failedTurn = conv.turns.find(
    (turn) =>
      turn.from === "user" &&
      turn.executionId === executionId &&
      turn.receipt === "sending",
  )
  const recoveredDraft =
    !uncertain && conv.draft.length === 0 && failedTurn?.from === "user"
      ? reupload
        ? notUploaded(failedTurn.content)
        : failedTurn.content
      : conv.draft
  const otherWork =
    conv.remote?.running ||
    !!conv.remote?.pending.length ||
    conv.turns.some(
      (turn) =>
        turn.from === "user" &&
        turn.executionId !== executionId &&
        ["sending", "accepted", "queued", "unknown"].includes(turn.receipt),
    )
  return replaceConversation(tabs, {
    ...(uncertain || otherWork ? conv : { ...conv, phase: "idle" as const }),
    error: detail,
    draft: recoveredDraft,
    draftReset:
      recoveredDraft === conv.draft ? conv.draftReset : (conv.draftReset ?? 0) + 1,
    turns: conv.turns.map((turn) =>
      turn.from === "user" &&
      turn.executionId === executionId &&
      turn.receipt === "sending"
        ? { ...turn, receipt: uncertain ? "unknown" : "failed", error: detail }
        : turn,
    ),
  })
}
