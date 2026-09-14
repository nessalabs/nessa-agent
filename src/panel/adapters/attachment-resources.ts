import type { FileAttachment } from "../../conversation"

/** Bound retained binary resources across all conversation drafts in one app. */
export const MAX_SESSION_ATTACHMENT_BYTES = 100 * 1024 * 1024

/** Object URLs retain browser-managed binary bytes, never base64 strings in Redux. */
export function createAttachmentResources() {
  const entries = new Map<string, { url: string; size: number }>()
  let bytes = 0
  function retain(ids: ReadonlySet<string>) {
    for (const [id, entry] of entries) {
      if (ids.has(id)) continue
      URL.revokeObjectURL(entry.url)
      entries.delete(id)
      bytes -= entry.size
    }
  }
  return {
    /** Admission is atomic: a rejected batch allocates no URLs. */
    add(files: readonly File[]): FileAttachment[] {
      const selectedBytes = files.reduce((total, file) => total + file.size, 0)
      if (bytes + selectedBytes > MAX_SESSION_ATTACHMENT_BYTES)
        throw new Error("Attachment session budget exceeded")
      const previous = new Set(entries.keys())
      try {
        return files.map((file) => {
          const id = crypto.randomUUID()
          const previewUrl = URL.createObjectURL(file)
          entries.set(id, { url: previewUrl, size: file.size })
          bytes += file.size
          return {
            type: "file",
            id,
            name: file.name,
            mimeType: file.type || "application/octet-stream",
            size: file.size,
            previewUrl,
          }
        })
      } catch (error) {
        retain(previous)
        throw error
      }
    },
    canAdd(size: number) {
      return bytes + size <= MAX_SESSION_ATTACHMENT_BYTES
    },
    retain,
  }
}

export type AttachmentResources = ReturnType<typeof createAttachmentResources>
