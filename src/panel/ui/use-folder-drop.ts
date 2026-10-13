import * as React from "react"
import {
  FolderDropEmptyError,
  FolderDropLimitError,
  readDroppedFolder,
} from "../adapters/dropped-folder"
import type { AttachmentRefusal } from "../application/attachment-notice"
import type { AttachmentDestination } from "./use-file-attachments"

/** Preserve the receiving draft and block its submission until traversal settles. */
export function useFolderDrop(
  destination: AttachmentDestination,
  addFiles: (files: readonly File[], destination: AttachmentDestination) => void,
  /** Named, not assumed: a walk can finish after somebody has changed tabs. */
  refuse: (refusal: AttachmentRefusal, destination: AttachmentDestination) => void,
) {
  const pending = React.useRef<{
    controller: AbortController
    destination: AttachmentDestination
  } | null>(null)
  React.useEffect(() => () => pending.current?.controller.abort(), [])
  function addFolderEntries(entries: FileSystemEntry[]) {
    if (
      pending.current?.destination.store === destination.store &&
      pending.current.destination.resources === destination.resources
    ) {
      refuse({ reason: "reading-folder" }, destination)
      return
    }
    const controller = new AbortController()
    const operation = { controller, destination }
    pending.current = operation
    void readDroppedFolder(entries, 20, controller.signal)
      .then((files) => {
        if (!controller.signal.aborted) addFiles(files, destination)
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted)
          refuse(
            {
              reason:
                error instanceof FolderDropEmptyError
                  ? "empty-folder"
                  : error instanceof FolderDropLimitError
                    ? "folder-too-large"
                    : "unreadable-folder",
            },
            destination,
          )
      })
      .finally(() => {
        if (pending.current === operation) pending.current = null
      })
  }
  return {
    addFolderEntries,
    isPending: (id: string) =>
      pending.current?.destination.store === destination.store &&
      pending.current.destination.resources === destination.resources &&
      pending.current.destination.conversationId === id,
  }
}
