import * as React from "react"
import { droppedImageUrl } from "../adapters/dropped-image"
import { droppedFolderEntries } from "../adapters/dropped-folder"
import { droppedText } from "../adapters/dropped-text"
import { useDropNavigationGuard } from "../adapters/use-drop-navigation-guard"

type DropActions = {
  addFolderEntries: (entries: FileSystemEntry[]) => void

  addImageUrl: (url: string) => Promise<void>
  focusComposer: () => void
  pasteAttachment: (text: string) => void
}

/** Route external content while leaving actual file payloads to FileDropZone. */
export function createContentDropHandlers(
  actions: DropActions,
  setDragging: (dragging: boolean) => void,
) {
  return {
    onDragEnterCapture(event: React.DragEvent<HTMLDivElement>) {
      if (
        event.dataTransfer.types.some((type) =>
          ["Files", "text/plain", "text/uri-list", "text/html"].includes(type),
        )
      ) {
        // WKWebView requires acceptance here as well as during dragover.
        event.preventDefault()
        setDragging(true)
      }
    },
    onDragLeaveCapture(event: React.DragEvent<HTMLDivElement>) {
      if (!event.currentTarget.contains(event.relatedTarget as Node | null))
        setDragging(false)
    },
    onDragOverCapture(event: React.DragEvent<HTMLDivElement>) {
      if (
        !event.dataTransfer.types.includes("Files") &&
        event.dataTransfer.types.some((type) =>
          ["text/plain", "text/uri-list", "text/html"].includes(type),
        )
      ) {
        event.preventDefault()
        event.dataTransfer.dropEffect = "copy"
      }
    },
    onDropCapture(event: React.DragEvent<HTMLDivElement>) {
      setDragging(false)
      const entries = droppedFolderEntries(event.dataTransfer)
      if (entries) {
        event.preventDefault()
        event.stopPropagation()
        actions.addFolderEntries(entries)
        return
      }
      // Native drags can advertise Files without providing any file bytes.
      if (event.dataTransfer.files.length > 0) return
      const imageUrl = droppedImageUrl(event.dataTransfer)
      if (imageUrl) {
        event.preventDefault()
        event.stopPropagation()
        void actions.addImageUrl(imageUrl)
        return
      }
      const text = droppedText(event.dataTransfer)
      if (!text) return
      event.preventDefault()
      event.stopPropagation()
      actions.focusComposer()
      actions.pasteAttachment(text)
    },
  }
}

/** Own drop feedback and acceptance, including the webview navigation guard. */
export function useContentDrop(actions: DropActions) {
  const [dragging, setDragging] = React.useState(false)
  useDropNavigationGuard()
  return { dragging, handlers: createContentDropHandlers(actions, setDragging) }
}
