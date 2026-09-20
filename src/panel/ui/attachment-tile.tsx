import { LoaderCircle, RotateCw, X } from "lucide-react"
import { ChatAttachmentTile } from "@nessa-ui/react/chat-bubbles"
import type { FileAttachment } from "../../conversation"
import { uploadFailureText, worthRetrying } from "../application/upload-image"
import { AttachmentIcon } from "./attachment-icon"

const cornerButton =
  "absolute inline-flex size-5 items-center justify-center rounded-full bg-background text-foreground shadow-sm outline-none focus-visible:[outline-style:solid] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring [&_svg]:size-3"

/**
 * One draft file: its preview, where its upload stands, and what can be done
 * about it. Renders; the hooks above it decide.
 *
 * The upload state is on the tile because that is where somebody is looking
 * when they wonder why send is waiting. An upload in flight marks the tile busy
 * and dims it; a failed one says why, and offers the retry right there when
 * trying the same bytes again could end differently — not when the gateway has
 * said it cannot read or fit this image. Remove stays available throughout:
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
  const uploading = file.upload.status === "uploading"
  const failure = file.upload.status === "failed" ? file.upload.reason : undefined
  return (
    <span
      className="relative m-1 inline-flex"
      data-upload={file.upload.status}
      aria-busy={uploading || undefined}
    >
      <ChatAttachmentTile
        label={file.name}
        imageSrc={file.mimeType.startsWith("image/") ? file.previewUrl : undefined}
        icon={<AttachmentIcon name={file.name} mimeType={file.mimeType} />}
        onOpen={onOpen}
        className={
          uploading ? "opacity-60" : failure ? "ring-2 ring-destructive" : undefined
        }
      />
      {uploading && (
        <span
          role="status"
          aria-label={`Uploading ${file.name}`}
          className="pointer-events-none absolute inset-0 flex items-center justify-center text-foreground [&_svg]:size-4"
        >
          <LoaderCircle aria-hidden="true" className="animate-spin" />
        </span>
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
