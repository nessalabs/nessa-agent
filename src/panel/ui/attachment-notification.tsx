import { Paperclip, Plus } from "lucide-react"
import { AgentNotification } from "@nessa-ui/react/agent-notification"
import type { AttachmentNotice } from "../application/attachment-notice"

/**
 * Everything the panel has to say about the draft's files, on the surface the
 * rest of this pane says things on.
 *
 * The same notification carries the connection's notices, the conversation's and
 * the update's, and `state` is its connection vocabulary: "disconnected" is the
 * only one of the four that renders the primary action at all, so a notice with
 * something to do declares itself disconnected whatever it is about. The glyph,
 * the heading, the line, and the labels here are all ours — a paperclip rather
 * than the state's own broken-aerial, because none of this is about a
 * connection.
 *
 * What is said, in what order, and whether there is anything to be done about it
 * is decided in `attachment-notice`. This maps the one action it chose onto the
 * component: retrying the uploads it named, or opening the file picker — which
 * is offered only where choosing the file again could end differently, never for
 * a file the picker would refuse for the same reason. A refusal can be put away;
 * a fact about the draft's own files cannot, because the notice is that fact.
 */
export function AttachmentNotification({
  notice,
  onRetryUploads,
  onChooseFiles,
  onDismiss,
}: {
  notice: AttachmentNotice
  onRetryUploads: (files: readonly string[]) => void
  onChooseFiles: () => void
  onDismiss: () => void
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
      onDismiss={notice.dismissible ? onDismiss : undefined}
    />
  )
}
