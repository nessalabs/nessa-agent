import * as React from "react"
import { RotateCw, X } from "lucide-react"
import { ChatAttachmentTile } from "@nessa-ui/react/chat-bubbles"
import {
  isImageFile,
  previewableImage,
  uploadFailureText,
  worthRetrying,
  type FileAttachment,
} from "../../conversation"
import { AttachmentIcon } from "./attachment-icon"

const cornerButton =
  "absolute inline-flex size-5 items-center justify-center rounded-full bg-background text-foreground shadow-sm outline-none focus-visible:[outline-style:solid] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring [&_svg]:size-3"

/**
 * One draft file: its preview, where its upload stands, and what can be done
 * about it. Renders; the hooks above it decide.
 *
 * The upload state is on the tile because that is where somebody is looking
 * when they wonder why send is waiting, and it is drawn on the tile's edge
 * because the picture in the middle is what they are looking at. An image
 * waiting its turn is dimmed, one in flight has a band of light going round it,
 * and one the gateway has taken says so once, in green, and goes back to being
 * an ordinary tile. A failed one says why, and offers the retry right there
 * when trying the same bytes again could end differently — not when the gateway
 * has said it cannot read or fit this image. Remove stays available throughout:
 * taking a tile away mid-upload is allowed, and the upload finding nothing to
 * report to is the design.
 */
export function AttachmentTile({
  file,
  onOpen,
  onRemove,
  onRetry,
}: {
  file: FileAttachment
  onOpen: () => void
  onRemove: () => void
  onRetry: () => void
}) {
  const status = file.upload.status
  const uploading = status === "uploading"
  // Only a few uploads run at once; an image that has not started is in line.
  const waiting = status === "not-started" && isImageFile(file.mimeType)
  const failure = file.upload.status === "failed" ? file.upload.reason : undefined
  // Green is for the moment an upload finishes, not for as long as the file is
  // stored: a tile coming back on screen — a tab returned to, a draft restored
  // — is a file that is ready, and says that by being plain. So the band is
  // owned by the change from uploading to stored, which is a thing that
  // happens rather than a thing that is true, and is read by comparing this
  // render's status with the last one's.
  const [seen, setSeen] = React.useState(status)
  const [finished, setFinished] = React.useState(false)
  if (seen !== status) {
    setSeen(status)
    setFinished(seen === "uploading" && status === "stored")
  }
  const ring = uploading ? "uploading" : finished ? "settled" : undefined
  return (
    <span
      className="relative m-1 inline-flex"
      data-upload={file.upload.status}
      aria-busy={uploading || undefined}
    >
      <ChatAttachmentTile
        label={file.name}
        // HEIC, RAW and the like are images the gateway reads and a webview
        // cannot: a named icon tile, not a broken picture.
        imageSrc={previewableImage(file.mimeType) ? file.previewUrl : undefined}
        icon={<AttachmentIcon name={file.name} mimeType={file.mimeType} />}
        onOpen={onOpen}
        // Dimmed only while it waits its turn. An upload under way has the
        // band, and dimming as well would hide the picture for the whole of it.
        className={
          waiting ? "opacity-60" : failure ? "ring-2 ring-destructive" : undefined
        }
      />
      {waiting && (
        <span
          role="status"
          aria-label={`${file.name} is waiting to upload`}
          title="Waiting to upload"
          className="pointer-events-none absolute inset-0"
        />
      )}
      {ring && (
        <span
          // The band's own removal: the green says its piece and ends, and the
          // element goes when the animation that carried it is over, so what is
          // painted and what this component holds cannot disagree. The sweep
          // never ends, so it never removes itself.
          data-state={ring}
          onAnimationEnd={finished ? () => setFinished(false) : undefined}
          role={uploading ? "status" : undefined}
          aria-label={uploading ? `Uploading ${file.name}` : undefined}
          className="nessa-upload-ring rounded-xl"
        />
      )}
      {failure && !worthRetrying(failure) && (
        <span
          role="status"
          aria-label={`${file.name} did not upload: ${uploadFailureText(failure)}`}
          title={`Did not upload: ${uploadFailureText(failure)}`}
          className="pointer-events-none absolute inset-0"
        />
      )}
      {failure && worthRetrying(failure) && (
        <button
          type="button"
          aria-label={`Retry uploading ${file.name}: ${uploadFailureText(failure)}`}
          title={`Did not upload: ${uploadFailureText(failure)}. Retry`}
          onClick={onRetry}
          className={`${cornerButton} -bottom-1.5 -right-1.5`}
        >
          <RotateCw aria-hidden="true" />
        </button>
      )}
      <button
        type="button"
        aria-label={`Remove ${file.name}`}
        title={`Remove ${file.name}`}
        onClick={onRemove}
        className={`${cornerButton} -right-1.5 -top-1.5`}
      >
        <X aria-hidden="true" />
      </button>
    </span>
  )
}
