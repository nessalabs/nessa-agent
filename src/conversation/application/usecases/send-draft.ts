import {
  contentText,
  messageFiles,
  messageImages,
  MAX_SEND_FILES,
  MAX_SEND_IMAGES,
  MAX_SEND_TOTAL_IMAGE_BYTES,
  type CommandFailure,
  type Conversation,
  type ImageRefusal,
  type MessageContent,
  type UserTurn,
} from "../../model"
import { findConversation, replaceConversation, takeTurnId } from "../internal/ids"
import { notUploaded } from "./release-uploads"
import type { LocalTabs } from "../local-tabs"

/**
 * Retain each local submission independently; busy work does not reject a queueable draft.
 *
 * `content` is the whole message: its text and its files. It becomes a turn only
 * if every file in it can go — as an image reference, or as a path the agent is
 * pointed at — so a turn never shows a file that was not sent. A message of
 * images alone, or of files alone, is a message.
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
  const attached = sendable.images.length + messageFiles(input.content).length
  if (!text.trim() && attached === 0) return tabs
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
    // Not renamed here: the gateway derives a conversation's title from its
    // first message and the view carries it (see `applyView`), so the tab and
    // the Messages list say the same thing by one rule.
    turns: [...conv.turns, userTurn],
    draft: [],
    phase: "thinking",
    pending: "",
    error: undefined,
    failure: undefined,
  })
}

/**
 * What to tell somebody whose files kept a draft from sending. The draft is
 * always kept.
 *
 * `canChoosePaths` is whether this surface has a picker that can say where a
 * file is, and only one sentence turns on it — the same condition the composer's
 * own notice about that file turns on, and for the same reason. Sending
 * somebody to `+` where `+` is a browser's file input is sending them to be
 * refused again in the same words, and the notice above the composer would be
 * saying the opposite of this at the same moment.
 */
export function imageRefusalMessage(
  refusal: ImageRefusal,
  canChoosePaths: boolean,
): string {
  switch (refusal.kind) {
    case "unsupported-file":
      // A file that is not an image and that nothing could say the location of.
      // In the app that is now only a paste: the host owns the drag as well as
      // the picker, so a dropped file arrives with its path like a picked one.
      // In a browser nothing has a location and there is no route at all,
      // which is the only honest thing to say there.
      return canChoosePaths
        ? `"${refusal.name}" cannot be sent: pasted bytes have no location, and the agent needs one. Drop the file on Nessa or choose it with +, or remove it to send.`
        : `"${refusal.name}" cannot be sent: a browser never says where a file is, and the agent needs that. Remove it to send, or send it from the Nessa app.`
    case "upload-failed":
      return `"${refusal.name}" did not upload; its tile says why. Retry it there or remove it, then send again.`
    case "upload-in-flight":
      return "Images are still uploading. Send again once they finish."
    case "too-many-images":
      return `A message carries up to ${MAX_SEND_IMAGES} images. Remove some to send.`
    case "images-too-large":
      return `A message carries up to ${MAX_SEND_TOTAL_IMAGE_BYTES / (1024 * 1024)} MiB of images in total, as the gateway stores them. Remove some to send.`
    case "too-many-files":
      return `A message points at up to ${MAX_SEND_FILES} files. Remove some to send.`
  }
}

/**
 * What to tell somebody whose message the gateway refused before taking it.
 * One sentence per refusal, chosen by its typed reason.
 *
 * Six answer undefined, and the panel shows the client's own message instead.
 * For `agent-not-configured`, `agent-unsupported`,
 * `conversations-not-configured` and `agent-startup-deadline` that message
 * names the remedy at length — unlike a control, a refused message does get a
 * sentence of its own from the client. `invalid-request` has nothing better to
 * say than the client already did about the arguments it refused. And
 * `attachment-cleanup-unavailable` is a close's news, which no message is ever
 * refused with; it is named only so the gateway's vocabulary stays accounted
 * for here.
 * `not-connected` is a control's word for a command that never left the window;
 * a message's own offline answer is the client's sentence, already worded for
 * a message.
 */
export function submissionRefusalMessage(reason: CommandFailure): string | undefined {
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
    case "conversation-state-unreadable":
      return "Nessa cannot read this conversation's saved state, so the message was not sent. It is back in the draft; start a new conversation to send it."
    case "conversation-deleted":
      return "This conversation was deleted, so the message was not sent. It is back in the draft; start a new conversation to send it."
    case "agent-not-configured":
    case "agent-unsupported":
    case "conversations-not-configured":
    case "agent-startup-deadline":
    case "not-connected":
    case "invalid-request":
    case "attachment-cleanup-unavailable":
    case "conversation-erasure-incomplete":
    case "deletion-unrecorded":
      return undefined
  }
}

/** Whether a refusal means the message's stored images are gone and must be uploaded again. */
export function refusalReleasesImages(reason: CommandFailure): boolean {
  return reason === "attachment-not-found" || reason === "attachment-unavailable"
}

/**
 * Why a draft was not sent, in the shape the panel needs to act on it.
 *
 * `kind` is what a caller branches on; `message` is what the conversation shows,
 * and only `empty-draft` has none — there is nothing to say about nothing.
 * `askAgain` means the decline is "not known yet" rather than "no", so the
 * conversation is worth reading again before somebody presses send a second time.
 */
export type DraftDecline = {
  kind: string
  message?: string
  askAgain?: boolean
}

/** What a send would carry: the caller's prose, and the draft's own files. */
export function draftMessage(
  conv: Conversation,
  content: MessageContent,
): MessageContent {
  return [
    ...content.filter((part) => part.type === "text" || part.type === "pasted-text"),
    ...conv.draft.filter((part) => part.type === "file"),
  ]
}

/**
 * Why this draft cannot be sent, or null when it can.
 *
 * Every local reason a send is declined that has somewhere to be said, in one
 * pure function, in the order somebody would want to hear them: there is no
 * session, the caller named a file this draft does not hold, the files cannot
 * go, there is nothing to say, the text is too long, and finally what the agent
 * takes. The caller shows and rejects in one place, so "was this draft taken"
 * has one answer wherever it is asked. A send into a conversation that is no
 * longer open is the caller's own: there is nowhere to show a sentence.
 *
 * `connected` is what the caller knows about the session and this cannot see.
 * A caller with no session to consult leaves it out; the effects' own
 * unavailable error is what then says so.
 */
export function declineReason(
  conv: Conversation,
  input: { content: MessageContent; connected?: boolean },
  canChoosePaths: boolean,
): DraftDecline | null {
  if (input.connected === false)
    return {
      kind: "not-connected",
      message: "Not connected to the gateway yet. Your draft has been kept.",
    }
  // Files are the draft's. Their upload state lives there and nowhere else, so
  // a caller's copy of a file part is never what gets sent — and one the draft
  // does not hold is refused rather than dropped from the message.
  const held = new Set(
    conv.draft.flatMap((part) => (part.type === "file" ? [part.id] : [])),
  )
  if (input.content.some((part) => part.type === "file" && !held.has(part.id)))
    return {
      kind: "unknown-attachment",
      message: "An attachment is no longer in this draft. Attach it again.",
    }
  const content = draftMessage(conv, input.content)
  const sendable = messageImages(content)
  if (!sendable.ok)
    return {
      kind: sendable.refusal.kind,
      message: imageRefusalMessage(sendable.refusal, canChoosePaths),
    }
  const text = contentText(content)
  // Refused rather than silently fulfilled, so every way this declines a draft
  // looks the same from outside: a rejection carrying a reason. A caller that
  // has to know whether the draft left — the composer deciding whether its
  // full-pane editor is finished with — cannot tell "nothing to send" from
  // "sent" otherwise. An image or a file is something to say: only a draft
  // with none of the three is empty.
  if (!text.trim() && sendable.images.length === 0 && messageFiles(content).length === 0)
    return { kind: "empty-draft" }
  if (new TextEncoder().encode(text).length > 8192)
    return {
      kind: "message-too-large",
      message:
        "This gateway accepts up to 8 KiB of text per message. Your draft has been kept.",
    }
  if (sendable.images.length === 0) return null
  // Whether the agent takes images is the gateway's fact, reported in a view,
  // and false until an agent is open. Staging the images created the
  // conversation and started its reads, so the answer is normally here by now.
  // When it is not, the draft waits: guessing yes would hand the gateway a
  // message it must refuse, and guessing no would refuse images an agent takes.
  const imageInput = conv.remote?.capabilities.imageInput
  if (imageInput === undefined)
    return {
      kind: "image-input-unknown",
      message: "Still checking whether this agent takes images. Send again in a moment.",
      askAgain: true,
    }
  if (!imageInput)
    return {
      kind: "image-input-unsupported",
      message:
        "This agent does not take images. Remove them to send; your draft has been kept.",
    }
  return null
}

/**
 * Whether a message went, as one answer rather than two flags.
 *
 * `uncertain` is a transport failure, which is no proof that admission failed:
 * the turn keeps its identities for an explicit retry and the draft stays
 * empty. `refused` is the gateway or the client saying the message was not
 * taken, so the draft comes back — and `reupload` is the refusal that also says
 * the turn's stored images are gone from the gateway, in which case the
 * recovered draft holds them as not started and the panel uploads them anew.
 */
export type SendOutcome = { kind: "uncertain" } | { kind: "refused"; reupload: boolean }

/** Record how a submission ended on its turn, and recover the draft when it can. */
export function failSend(
  tabs: LocalTabs,
  conversationId: string,
  executionId: string,
  detail: string,
  outcome: SendOutcome,
  failure?: CommandFailure,
): LocalTabs {
  const uncertain = outcome.kind === "uncertain"
  const reupload = outcome.kind === "refused" && outcome.reupload
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
    failure,
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
