import {
  AttachmentResource,
  type AttachmentResourceUsage,
} from "../../conversation/resources"
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
  const entries = new Map<AttachmentResource, Blob>()
  let previewOnly = new Set<AttachmentResource>()
  /** Bytes that count against attaching: every retained resource not classified as preview-only. */
  const budgeted = () => {
    let total = 0
    for (const resource of entries.keys())
      if (!previewOnly.has(resource)) total += resource.size
    return total
  }
  function release(resource: AttachmentResource) {
    const entry = entries.get(resource)
    if (!entry) return
    URL.revokeObjectURL(resource.previewUrl)
    entries.delete(resource)
  }
  function retain(usage: AttachmentResourceUsage) {
    previewOnly = new Set(usage.previews)
    for (const resource of entries.keys()) {
      if (usage.needed.has(resource) || previewOnly.has(resource)) continue
      release(resource)
    }
  }
  return {
    /** Budget rejection allocates no URLs; partial allocation releases its own URLs. */
    add(files: readonly File[]): AttachmentResource[] {
      const selected: Array<{
        bytes: Blob
        name: string
        mimeType: string
        size: number
      }> = []
      const count = files.length
      for (let index = 0; index < count; index++) {
        const file = files[index]
        if (!file) throw new TypeError("Missing attachment bytes")
        const name = file.name
        const readType = Object.getOwnPropertyDescriptor(Blob.prototype, "type")?.get
        if (!readType) throw new TypeError("Missing native Blob type accessor")
        const mimeType = readType.call(file) as string
        // Capture native immutable bytes, not a caller's later shadow Blob hooks.
        const bytes = Object.freeze(
          Blob.prototype.slice.call(file, 0, undefined, mimeType),
        ) as Blob
        selected.push({ bytes, name, mimeType, size: bytes.size })
      }
      const selectedBytes = selected.reduce((total, file) => total + file.size, 0)
      if (budgeted() + selectedBytes > MAX_SESSION_ATTACHMENT_BYTES)
        throw new Error("Attachment session budget exceeded")
      const previous = new Set(entries.keys())
      try {
        return selected.map((file) => {
          const previewUrl = URL.createObjectURL(file.bytes)
          try {
            const resource = AttachmentResource.create({
              name: file.name,
              mimeType: file.mimeType,
              size: file.size,
              previewUrl,
              path: null,
            })
            entries.set(resource, file.bytes)
            return resource
          } catch (error) {
            URL.revokeObjectURL(previewUrl)
            throw error
          }
        })
      } catch (error) {
        for (const resource of entries.keys())
          if (!previous.has(resource)) release(resource)
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
    addChosen(chosen: readonly ChosenFile[]): AttachmentResource[] {
      const resources: AttachmentResource[] = []
      const count = chosen.length
      for (let index = 0; index < count; index++) {
        const file = chosen[index]
        if (!file) throw new TypeError("Missing attachment path")
        const { name, mimeType, size, path } = file
        resources.push(
          AttachmentResource.create({ name, mimeType, size, previewUrl: "", path }),
        )
      }
      return resources
    },
    canAdd(size: number) {
      return budgeted() + size <= MAX_SESSION_ATTACHMENT_BYTES
    },
    /** The bytes behind a retained file, or nothing once it has been released. */
    bytes(resource: AttachmentResource): Blob | undefined {
      return entries.get(resource)
    },
    /** Reconcile one store's current resource references; needed bytes remain budgeted. */
    retain,
  }
}

export type AttachmentResources = ReturnType<typeof createAttachmentResources>
