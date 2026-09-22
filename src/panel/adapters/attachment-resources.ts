import type { FileAttachment } from "../../conversation"
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
 * The budget is for attaching. A file whose message the gateway has taken is
 * still retained — the transcript paints it — but no longer counts, because it
 * can never come back to a draft and the conversation bounds how many of those
 * it keeps. Otherwise a few sent messages would refuse the next attachment
 * while every draft sat empty.
 *
 * A file that arrived by a route with no path and is not an image can never be
 * sent, and its bytes are kept and counted anyway. That is deliberate: they
 * back the preview somebody opens to decide whether it was the file they
 * meant, and removing the tile hands the budget straight back. Dropping the
 * bytes instead would leave a tile that cannot be opened, for a file that
 * cannot be sent, which is a worse thing to be holding.
 */
export function createAttachmentResources() {
  const entries = new Map<
    string,
    { url: string; size: number; bytes: Blob; sent: boolean }
  >()
  /** Bytes that count against attaching: everything retained that is not sent. */
  const budgeted = () => {
    let total = 0
    for (const entry of entries.values()) if (!entry.sent) total += entry.size
    return total
  }
  function retain(ids: ReadonlySet<string>, sent: ReadonlySet<string> = new Set()) {
    for (const [id, entry] of entries) {
      if (ids.has(id)) {
        // One way only: a sent file is never a draft file again.
        if (sent.has(id)) entry.sent = true
        continue
      }
      URL.revokeObjectURL(entry.url)
      entries.delete(id)
    }
  }
  return {
    /** Admission is atomic: a rejected batch allocates no URLs. */
    add(files: readonly File[]): FileAttachment[] {
      const selectedBytes = files.reduce((total, file) => total + file.size, 0)
      if (budgeted() + selectedBytes > MAX_SESSION_ATTACHMENT_BYTES)
        throw new Error("Attachment session budget exceeded")
      const previous = new Set(entries.keys())
      try {
        return files.map((file) => {
          const id = crypto.randomUUID()
          const previewUrl = URL.createObjectURL(file)
          entries.set(id, { url: previewUrl, size: file.size, bytes: file, sent: false })
          return {
            type: "file",
            id,
            name: file.name,
            // As the browser reports it, which for most camera RAW files is
            // not at all. What such a file is declared as is the conversation's
            // rule, applied when it is attached.
            mimeType: file.type || "application/octet-stream",
            size: file.size,
            previewUrl,
            upload: { status: "not-started" },
            // A browser `File` deliberately does not say where it came from.
            // What can learn that is the host's picker; see `addChosen`.
            path: null,
          }
        })
      } catch (error) {
        retain(previous)
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
    addChosen(chosen: readonly ChosenFile[]): FileAttachment[] {
      return chosen.map((file) => ({
        type: "file",
        id: crypto.randomUUID(),
        name: file.name,
        // The platform's answer, in the same slot a dropped file's `type`
        // goes, so the conversation's one naming rule sees the same inputs
        // whichever route a file arrived by. Empty where the system had none,
        // which is exactly what a browser reporting nothing looks like.
        mimeType: file.mimeType || "application/octet-stream",
        size: file.size,
        previewUrl: "",
        upload: { status: "not-started" },
        path: file.path,
      }))
    },
    canAdd(size: number) {
      return budgeted() + size <= MAX_SESSION_ATTACHMENT_BYTES
    },
    /** The bytes behind a retained file, or nothing once it has been released. */
    bytes(id: string): Blob | undefined {
      return entries.get(id)?.bytes
    },
    /**
     * Keep exactly these files and release the rest. `sent` names the kept files
     * whose message the gateway has taken; they stop counting against attaching.
     */
    retain,
  }
}

export type AttachmentResources = ReturnType<typeof createAttachmentResources>
