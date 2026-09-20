import {
  isImageFile,
  validDraftAttachments,
  validImageReference,
  type FileAttachment,
} from "../../model"
import type { LocalTabs } from "../local-tabs"
import type { UploadChange } from "../ports"
import { findConversation, replaceConversation, withDraft } from "../internal"

/**
 * Append files to the requested conversation, atomically within limits.
 *
 * Every file arrives with its upload not started, whatever the caller wrote on
 * it: only `changeUpload` moves that state, so a part cannot be attached already
 * claiming bytes the gateway never received.
 */
export function attachFiles(
  tabs: LocalTabs,
  files: FileAttachment[],
  conversationId: string,
): LocalTabs {
  const current = findConversation(tabs, conversationId)
  if (!current || files.length === 0) return tabs
  const draft = [
    ...current.draft,
    ...files.map((file) => ({ ...file, upload: { status: "not-started" as const } })),
  ]
  if (!validDraftAttachments(draft)) return tabs
  return replaceConversation(tabs, withDraft(current, draft))
}

/** Remove a file preview from the active conversation without changing prose. */
export function removeFile(tabs: LocalTabs, id: string): LocalTabs {
  const current = findConversation(tabs, tabs.activeId)
  if (!current || !current.draft.some((part) => part.type === "file" && part.id === id))
    return tabs
  return replaceConversation(
    tabs,
    withDraft(
      current,
      current.draft.filter((part) => part.type !== "file" || part.id !== id),
    ),
  )
}

/**
 * The one owner of a draft file's upload state.
 *
 * It only ever changes a file that is still in a draft. An upload that settles
 * after its tile was removed finds nothing here and changes nothing — it cannot
 * put the file back — and a sent turn's files are past changing.
 *
 * Each step has one predecessor. A result for an upload that is not in flight
 * is ignored rather than applied: `stored` arriving on a failed or retried part
 * would describe a different attempt from the one on screen.
 */
export function changeUpload(tabs: LocalTabs, change: UploadChange): LocalTabs {
  const current = tabs.conversations.find((conversation) =>
    conversation.draft.some((part) => part.type === "file" && part.id === change.fileId),
  )
  const file = current?.draft.find(
    (part): part is FileAttachment => part.type === "file" && part.id === change.fileId,
  )
  if (!current || !file) return tabs
  const next = changedFile(file, change)
  if (!next) return tabs
  return replaceConversation(
    tabs,
    withDraft(
      current,
      current.draft.map((part) => (part === file ? next : part)),
    ),
  )
}

function changedFile(
  file: FileAttachment,
  change: UploadChange,
): FileAttachment | undefined {
  const from = file.upload.status
  switch (change.to) {
    case "uploading":
      return from === "not-started"
        ? { ...file, upload: { status: "uploading" } }
        : undefined
    case "not-started":
      // Trying again. Only a failure has anything to try again.
      return from === "failed"
        ? { ...file, upload: { status: "not-started" } }
        : undefined
    case "failed":
      return from === "uploading"
        ? { ...file, upload: { status: "failed", reason: change.reason } }
        : undefined
    case "stored":
      if (from !== "uploading") return undefined
      // The reference is the gateway's and need not resemble the file: a HEIC
      // attached here is stored as something else, under another digest and
      // size. What it must be is a reference, on a file that was an image to
      // begin with — otherwise a message would name something nobody stored.
      if (!isImageFile(file.mimeType) || !validImageReference(change.image))
        return { ...file, upload: { status: "failed", reason: "rejected" } }
      return {
        ...file,
        upload: {
          status: "stored",
          image: {
            digest: change.image.digest,
            mimeType: change.image.mimeType,
            size: change.image.size,
          },
        },
      }
  }
}
