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
  const operations = React.useRef(
    new Set<{
      controller: AbortController
      destination: AttachmentDestination
    }>(),
  )
  const [active, setActive] = React.useState<
    ReadonlySet<{
      controller: AbortController
      destination: AttachmentDestination
    }>
  >(new Set())
  React.useEffect(
    () => () => {
      for (const operation of operations.current) operation.controller.abort()
      operations.current.clear()
    },
    [],
  )
  function addFolderEntries(entries: FileSystemEntry[]) {
    if (
      [...operations.current].some(
        (operation) =>
          operation.destination.store === destination.store &&
          operation.destination.resources === destination.resources,
      )
    ) {
      refuse({ reason: "reading-folder" }, destination)
      return
    }
    const controller = new AbortController()
    const operation = { controller, destination }
    operations.current.add(operation)
    setActive(new Set(operations.current))
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
        if (!operations.current.delete(operation)) return
        setActive(new Set(operations.current))
      })
  }
  return {
    addFolderEntries,
    isPending: (id: string) =>
      [...active].some(
        (operation) =>
          operation.destination.store === destination.store &&
          operation.destination.resources === destination.resources &&
          operation.destination.conversationId === id,
      ),
  }
}
