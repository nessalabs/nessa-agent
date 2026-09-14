import * as React from "react"
import {
  FolderDropEmptyError,
  FolderDropLimitError,
  readDroppedFolder,
} from "../adapters/dropped-folder"

/** Preserve the receiving draft and block its submission until traversal settles. */
export function useFolderDrop(
  conversationId: string,
  addFiles: (files: readonly File[], conversationId: string) => void,
  setError: (message: string) => void,
) {
  const pending = React.useRef<{
    controller: AbortController
    conversationId: string
  } | null>(null)
  React.useEffect(() => () => pending.current?.controller.abort(), [])
  function addFolderEntries(entries: FileSystemEntry[]) {
    if (pending.current) {
      setError("Please wait for the dropped folder to finish loading.")
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
          setError(
            error instanceof FolderDropEmptyError
              ? "The dropped folders contain no files to attach."
              : error instanceof FolderDropLimitError
                ? "Attach up to 20 files; folders must contain at most 1,000 entries."
                : "The dropped folder could not be read. Please select its files directly.",
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
