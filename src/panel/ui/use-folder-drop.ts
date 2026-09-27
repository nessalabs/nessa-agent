import * as React from "react"
import {
  FolderDropEmptyError,
  FolderDropLimitError,
  readDroppedFolder,
} from "../adapters/dropped-folder"
import type { AttachmentRefusal } from "../application/attachment-notice"

/** Preserve the receiving draft and block its submission until traversal settles. */
export function useFolderDrop(
  conversationId: string,
  addFiles: (files: readonly File[], conversationId: string) => void,
  /** Named, not assumed: a walk can finish after somebody has changed tabs. */
  refuse: (refusal: AttachmentRefusal, conversationId: string) => void,
) {
  const pending = React.useRef<{
    controller: AbortController
    conversationId: string
  } | null>(null)
  React.useEffect(() => () => pending.current?.controller.abort(), [])
  function addFolderEntries(entries: FileSystemEntry[]) {
    if (pending.current) {
      refuse({ reason: "reading-folder" }, conversationId)
      return
    }
    const controller = new AbortController()
    pending.current = { controller, conversationId }
    void readDroppedFolder(entries, 20, controller.signal)
      .then((files) => {
        if (!controller.signal.aborted) addFiles(files, conversationId)
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
            conversationId,
          )
      })
      .finally(() => {
        pending.current = null
      })
  }
  return {
    addFolderEntries,
    isPending: (id: string) => pending.current?.conversationId === id,
  }
}
