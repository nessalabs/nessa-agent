import { Paperclip, Plus } from "lucide-react"
import { AgentNotification } from "@nessa-ui/react/agent-notification"
import { isImageFile, type FileAttachment } from "../../conversation"
import {
  attachmentNotices,
  type AttachmentNotice,
  type AttachmentRefusal,
} from "../application/attachment-notice"

/**
 * One notice, on the surface the rest of this pane says things on.
 *
 * The same notification carries the connection's notices, the conversation's
 * and the update's, and `state` is its connection vocabulary: "disconnected" is
 * the only one of the four that renders the primary action at all, so a notice
 * with something to do declares itself disconnected whatever it is about. The
 * glyph, the heading, the line, and the labels here are all ours — a paperclip
 * rather than the state's own broken-aerial, because none of this is about a
 * connection.
 *
 * The one action the notice chose is mapped onto the component: retrying the
 * uploads it named, or opening the file picker — which is offered only where
 * choosing the file again could end differently, never for a file the picker
 * would refuse for the same reason.
 */
function AttachmentNotification({
  notice,
  onRetryUploads,
  onChooseFiles,
  onDismiss,
}: {
  notice: AttachmentNotice
  onRetryUploads: (files: readonly string[]) => void
  onChooseFiles: () => void
  /** Given only for a refusal; a fact about the draft is not somebody's to put away. */
  onDismiss?: () => void
}) {
  const action = notice.action
  return (
    <AgentNotification
      className="mb-2"
      state="disconnected"
      icon={Paperclip}
      title={notice.title}
      description={notice.description}
      retryIcon={action?.kind === "choose-files" ? Plus : undefined}
      retryLabel={action?.kind === "choose-files" ? "Choose files" : "Retry"}
      onRetry={
        action === null
          ? undefined
          : action.kind === "choose-files"
            ? onChooseFiles
            : () => onRetryUploads(action.files)
      }
      onDismiss={onDismiss}
    />
  )
}

/**
 * Everything the composer has to say about attachments, and the whole of it.
 *
 * This component exists so the deciding is not spread across the panel's
 * chrome. Which notices there are, and in which order, is
 * {@link attachmentNotices}; all that is left here is the wiring, so there is
 * nowhere above this to quietly gate one notice on another's absence — which is
 * the bug this replaced.
 *
 * Both notices render when both are true. Only the refusal can be dismissed.
 */
export function AttachmentNotices({
  refusal,
  files,
  imageInput,
  onRetryUploads,
  onChooseFiles,
  onDismissRefusal,
}: {
  refusal: AttachmentRefusal | null
  files: readonly FileAttachment[]
  /** Undefined while the gateway has not answered, which is not a no. */
  imageInput: boolean | undefined
  onRetryUploads: (files: readonly string[]) => void
  onChooseFiles: () => void
  onDismissRefusal: () => void
}) {
  const notices = attachmentNotices({
    refusal,
    files: files.map((file) => ({
      id: file.id,
      name: file.name,
      image: isImageFile(file.mimeType),
      upload: file.upload,
    })),
    imageInput,
  })
  return (
    <>
      {notices.map((notice) => (
        <AttachmentNotification
          key={notice.kind}
          notice={notice}
          onRetryUploads={onRetryUploads}
          onChooseFiles={onChooseFiles}
          onDismiss={notice.kind === "refusal" ? onDismissRefusal : undefined}
        />
      ))}
    </>
  )
}

/**
 * That a dropped image is being fetched, for somebody who cannot see the
 * placeholder tile saying so.
 *
 * Mounted always and empty when there is nothing to read, because that is the
 * difference between announcing and not: a live region inserted with its text
 * already in it is usually read by nothing, which is what was wrong with the
 * paragraph this replaces. The region is here first and the text arrives into
 * it. The visible half is the busy tile in the composer; this adds no chrome.
 */
export function AttachmentReadingStatus({ reading }: { reading: boolean }) {
  return (
    <span role="status" aria-live="polite" className="sr-only">
      {reading ? "Reading files…" : ""}
    </span>
  )
}
