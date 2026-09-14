import type { MessageContent } from "./content"

/** Local preview file; the current backend does not accept file payloads. */
export type FileAttachment = {
  type: "file"
  id: string
  name: string
  mimeType: string
  size: number
  previewUrl: string
}

export const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024
export const MAX_DRAFT_ATTACHMENT_BYTES = 50 * 1024 * 1024
export const MAX_DRAFT_ATTACHMENTS = 20

/** Enforce per-file and aggregate draft budgets before changing stored content. */
export function validDraftAttachments(content: MessageContent): boolean {
  const files = content.filter((part) => part.type === "file")
  return (
    files.length <= MAX_DRAFT_ATTACHMENTS &&
    new Set(files.map((file) => file.id)).size === files.length &&
    files.every(
      (file) =>
        Number.isSafeInteger(file.size) &&
        file.size >= 0 &&
        file.size <= MAX_ATTACHMENT_BYTES,
    ) &&
    files.reduce((bytes, file) => bytes + file.size, 0) <= MAX_DRAFT_ATTACHMENT_BYTES
  )
}

/** File previews cannot enter the text-only send path. */
export function hasFileAttachments(content: MessageContent): boolean {
  return content.some((part) => part.type === "file")
}
