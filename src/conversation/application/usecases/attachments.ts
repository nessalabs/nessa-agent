import {
  declaredMediaType,
  isImageFile,
  validDraftAttachments,
  validImageReference,
  type FileAttachment,
  type AttachmentInput,
} from "../../model"
import type { LocalTabs } from "../local-tabs"
import type { UploadChange } from "../ports"
import {
  findConversation,
  replaceConversation,
  withDraft,
  takePlacementId,
} from "../internal"

/**
 * Append files to the requested conversation, atomically within limits.
 *
 * Every file arrives with its upload not started, whatever the caller wrote on
 * it: only `changeUpload` moves that state, so a part cannot be attached already
 * claiming bytes the gateway never received.
 *
 * Its media type is settled here too, for every file however it arrived,
 * because the type is what decides the file's route and the gesture never is.
 * A browser reports most camera RAW files, and some HEIC, with no type at all,
 * and a file chosen through the host's picker was never opened by anything, so
 * in both cases the extension decides. Getting that wrong is how the same file
 * comes to behave differently depending on how it was attached.
 */
export function attachFiles(
  tabs: LocalTabs,
  files: AttachmentInput[],
  conversationId: string,
): LocalTabs {
  const current = findConversation(tabs, conversationId)
  const count = files.length
  if (!current || count === 0) return tabs
  // Validate the whole resource batch before consuming placement identities.
  const inputs: AttachmentInput[] = []
  for (let index = 0; index < count; index++) {
    const file = files[index]
    if (!file) return tabs
    const { resourceId, name, mimeType, size, previewUrl, path } = file
    inputs.push({
      type: "file",
      resourceId,
      name,
      mimeType: declaredMediaType(name, mimeType),
      size,
      previewUrl,
      path,
    })
  }
  const proposed = [...current.draft, ...inputs]
  if (!validDraftAttachments(proposed)) return tabs
  let allocated = tabs
  const admitted = inputs.map((file): FileAttachment => {
    const next = takePlacementId(allocated)
    allocated = next.tabs
    return { ...file, placementId: next.id, upload: { status: "not-started" } }
  })
  return replaceConversation(
    allocated,
    withDraft(current, [...current.draft, ...admitted]),
  )
}

/** Remove a file preview from the active conversation without changing prose. */
export function removeFile(tabs: LocalTabs, id: string): LocalTabs {
  const current = findConversation(tabs, tabs.activeId)
  if (
    !current ||
    !current.draft.some((part) => part.type === "file" && part.placementId === id)
  )
    return tabs
  return replaceConversation(
    tabs,
    withDraft(
      current,
      current.draft.filter((part) => part.type !== "file" || part.placementId !== id),
    ),
  )
}

/**
 * The one owner of a draft file's upload state.
 *
 * It only changes a file in its named parent draft. A completion whose parent
 * or file no longer exists changes nothing; a sent turn's files are past changing.
 *
 * Each step has one predecessor. A result for an upload that is not in flight
 * is ignored rather than applied: `stored` arriving on a failed part is not
 * a transition from the state on screen. Remove/re-add mints a new placement
 * (AL1/AL3 in attachment-resources/upload-targets tests). Retry within the same
 * live placement continues using these predecessor rules.
 */
export function changeUpload(tabs: LocalTabs, change: UploadChange): LocalTabs {
  const current = findConversation(tabs, change.conversationId)
  const file = current?.draft.find(
    (part): part is FileAttachment =>
      part.type === "file" && part.placementId === change.placementId,
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
