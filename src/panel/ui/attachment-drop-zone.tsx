import type * as React from "react"
import { FileDropZone } from "@nessa-ui/react/file-drop-zone"
import { MAX_ATTACHMENT_BYTES, MAX_DRAFT_ATTACHMENTS } from "../../conversation"
import {
  droppedFilesRefusal,
  type AttachmentRefusal,
} from "../application/attachment-notice"

/**
 * Files dropped anywhere on the panel, and what to say about the ones the drop
 * would not hand over.
 *
 * The zone reports which rule refused each file, and both halves of a drop are
 * answered: what passed goes to `onFiles`, and what did not becomes one
 * refusal with its own reason. That is the whole point of the wiring being a
 * component rather than four props on the panel's chrome — the refusal for an
 * oversized file is reached only through here, and it used to be one sentence
 * for every rule, ending "Try selecting them with +", which is advice the +
 * picker cannot honour: it applies this same `maxSize`.
 *
 * **This is the browser's drop path only.** In the app the host owns the drag
 * — it is the only thing that can learn a dropped file's path — and the page
 * receives no drop events at all, so nothing here fires; see `use-host-drop`.
 * It stays because a browser still has its own drops and they still work.
 *
 * `asChild`, so the zone adds no DOM: the child it merges onto is the panel.
 */
export function AttachmentDropZone({
  onFiles,
  onRefused,
  children,
}: {
  onFiles: (files: File[]) => void
  onRefused: (refusal: AttachmentRefusal) => void
  children: React.ReactNode
}) {
  return (
    <FileDropZone
      asChild
      onFiles={onFiles}
      onRejectedFiles={(rejections) => {
        const refusal = droppedFilesRefusal(rejections)
        if (refusal) onRefused(refusal)
      }}
      maxFiles={MAX_DRAFT_ATTACHMENTS}
      maxSize={MAX_ATTACHMENT_BYTES}
    >
      {children}
    </FileDropZone>
  )
}
