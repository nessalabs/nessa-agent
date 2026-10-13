import type { AttachmentInput, AttachmentResourceUsage } from "../../conversation"
import type { ChosenFile } from "../../host"

/**
 * Bound retained binary resources across all conversation drafts in one app.
 * MiB, like the per-file and per-draft budgets it sits above.
 */
export const MAX_SESSION_ATTACHMENT_BYTES = 256 * 1024 * 1024

/**
 * Object URLs retain browser-managed binary bytes, never base64 strings in Redux.
 *
 * The Blob is kept beside its URL because an upload needs the bytes and the URL
 * is no way to get them back: the packaged app's content policy lets an `<img>`
 * load a `blob:` URL and does not let `fetch` read one. Holding the reference
 * costs nothing — the URL already keeps the same bytes alive.
 *
 * The budget counts each retained resource once while any draft or unsettled
 * turn needs it. A resource held only for accepted previews does not count
 * against attaching. The application publishes that classification for all
 * placements together; one accepted placement cannot exempt another draft.
 *
 * A file that arrived by a route with no path and is not an image can never be
 * sent, and its bytes are kept and counted anyway. That is deliberate: they
 * back the preview somebody opens to decide whether it was the file they
 * meant, and removing the tile hands the budget straight back. Dropping the
 * bytes instead would leave a tile that cannot be opened, for a file that
 * cannot be sent, which is a worse thing to be holding.
 */
export function createAttachmentResources() {
  const entries = new Map<string, { url: string; size: number; bytes: Blob }>()
  let previewOnly = new Set<string>()
  /** Bytes that count against attaching: every retained resource not classified as preview-only. */
  const budgeted = () => {
    let total = 0
    for (const [id, entry] of entries) if (!previewOnly.has(id)) total += entry.size
    return total
  }
  function release(id: string) {
    const entry = entries.get(id)
    if (!entry) return
    URL.revokeObjectURL(entry.url)
    entries.delete(id)
  }
  function retain(usage: AttachmentResourceUsage) {
    previewOnly = new Set(usage.previews.keys())
    for (const id of entries.keys()) {
      if (usage.needed.has(id) || previewOnly.has(id)) continue
      release(id)
    }
  }
  return {
    /** Admission is atomic: a rejected batch allocates no URLs. */
    add(files: readonly File[]): AttachmentInput[] {
      const selectedBytes = files.reduce((total, file) => total + file.size, 0)
      if (budgeted() + selectedBytes > MAX_SESSION_ATTACHMENT_BYTES)
        throw new Error("Attachment session budget exceeded")
      const previous = new Set(entries.keys())
      try {
        return files.map((file) => {
          const resourceId = crypto.randomUUID()
          const previewUrl = URL.createObjectURL(file)
          entries.set(resourceId, { url: previewUrl, size: file.size, bytes: file })
          return {
            type: "file",
            resourceId,
            name: file.name,
            // As the browser reports it, which for most camera RAW files is
            // not at all. What such a file is declared as is the conversation's
            // rule, applied when it is attached.
            mimeType: file.type || "application/octet-stream",
            size: file.size,
            previewUrl,
            // A browser `File` deliberately does not say where it came from.
            // What can learn that is the host's picker; see `addChosen`.
            path: null,
          }
        })
      } catch (error) {
        for (const id of entries.keys()) if (!previous.has(id)) release(id)
        throw error
      }
    },
    /**
     * The files the host's picker chose that are travelling by path rather
     * than by content. Nothing is retained for them and nothing is counted
     * against the session budget: this window never holds their bytes, which
     * is the whole reason a path is worth having. They have no preview for the
     * same reason — the tile paints an icon.
     *
     * Only the ones that are not images reach this. An image is uploaded
     * wherever it came from, so the picker's images are read first and arrive
     * through `add` with their bytes, like any other image.
     */
    addChosen(chosen: readonly ChosenFile[]): AttachmentInput[] {
      return chosen.map((file) => ({
        type: "file",
        resourceId: crypto.randomUUID(),
        name: file.name,
        // The platform's answer, in the same slot a dropped file's `type`
        // goes, so the conversation's one naming rule sees the same inputs
        // whichever route a file arrived by. Empty where the system had none,
        // which is exactly what a browser reporting nothing looks like.
        mimeType: file.mimeType || "application/octet-stream",
        size: file.size,
        previewUrl: "",
        path: file.path,
      }))
    },
    canAdd(size: number) {
      return budgeted() + size <= MAX_SESSION_ATTACHMENT_BYTES
    },
    /** The bytes behind a retained file, or nothing once it has been released. */
    bytes(resourceId: string): Blob | undefined {
      return entries.get(resourceId)?.bytes
    },
    /** Reconcile one store's current resource references; needed bytes remain budgeted. */
    retain,
  }
}

export type AttachmentResources = ReturnType<typeof createAttachmentResources>
