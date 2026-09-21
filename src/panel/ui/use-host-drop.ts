import * as React from "react"
import { onAttachmentDragging, onAttachmentDropped, type ChosenFile } from "../../host"
import { droppedImageUrl } from "../adapters/dropped-image"
import { droppedText } from "../adapters/dropped-text"
import { pickerRefusal, type AttachmentRefusal } from "../application/attachment-notice"

type HostDropActions = {
  /** The same call the picker's answer goes through. */
  addChosenFiles: (chosen: readonly ChosenFile[]) => void
  addImageUrl: (url: string) => void
  focusComposer: () => void
  pasteAttachment: (text: string) => void
  refuse: (refusal: AttachmentRefusal) => void
}

/**
 * The three strings the host read off the drag pasteboard, wearing
 * `DataTransfer`'s face.
 *
 * So that `droppedText` and `droppedImageUrl` — which already decide, between
 * them, whether a drag is prose or a picture, and have tests of their own for
 * every awkward case — are used unchanged rather than reimplemented against a
 * second shape. They only ever call `getData`, which is the whole of what this
 * has to be.
 */
function asTransfer(text: {
  plain: string
  uriList: string
  html: string
}): Pick<DataTransfer, "getData"> {
  const flavours: Record<string, string> = {
    "text/plain": text.plain,
    "text/uri-list": text.uriList,
    "text/html": text.html,
  }
  return { getData: (type: string) => flavours[type] ?? "" }
}

/**
 * Drops, from the host rather than from the page.
 *
 * **The page receives no drop events at all any more.** `dragDropEnabled` is
 * on, which is the only way a dropped file's path can be known, and it costs
 * the webview every HTML5 drag event rather than only the ones carrying files
 * — see `src-tauri/src/attachments/dropping.rs` for the two vendored lines
 * that settle it. So everything that used to arrive through `DataTransfer`
 * arrives here: files, prose, and an image dragged off a web page.
 *
 * **A dropped file and a picked file are the same thing by the time they get
 * here.** Both are `ChosenFile`s the host described and ticketed, and both go
 * through `addChosenFiles`, which is where the type decides the route: an
 * image is read and uploaded, anything else travels as its path. There is no
 * second policy, and deliberately no second code path to drift from the first
 * — that drift is the defect this whole feature has already had once.
 *
 * The browser keeps its own `useContentDrop`: outside Tauri there is no host
 * to take the drag, the page still gets its events, and nothing here fires.
 */
export function useHostDrop(actions: HostDropActions) {
  const [dragging, setDragging] = React.useState(false)
  // The handlers are read at drop time rather than captured at subscribe time,
  // so a drop lands on the conversation that is open when it happens instead
  // of the one that was open when the panel mounted.
  const latest = React.useRef(actions)
  React.useLayoutEffect(() => {
    latest.current = actions
  })
  React.useEffect(() => {
    let live = true
    const subscriptions = Promise.all([
      onAttachmentDragging((over) => {
        if (live) setDragging(over)
      }),
      onAttachmentDropped((dropped) => {
        if (!live) return
        setDragging(false)
        if (dropped.refused) {
          latest.current.refuse(pickerRefusal(dropped.refused))
          return
        }
        if (dropped.files.length > 0) {
          latest.current.addChosenFiles(dropped.files)
          return
        }
        const transfer = asTransfer(dropped.text)
        const image = droppedImageUrl(transfer)
        if (image) {
          latest.current.addImageUrl(image)
          return
        }
        const text = droppedText(transfer)
        if (!text) return
        latest.current.focusComposer()
        latest.current.pasteAttachment(text)
      }),
    ])
    return () => {
      live = false
      void subscriptions.then((unlisten) => unlisten.forEach((stop) => stop()))
    }
  }, [])
  return { dragging }
}
