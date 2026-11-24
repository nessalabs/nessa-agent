import { IconButton } from "../../../ui/icon-button"

/**
 * Chat context's button in a session pane's header: shows and hides
 * everything about this chat in a column on the pane's right, inside the
 * chat's own space (`Pane`).
 */
export function ChatContextButton({
  shown,
  controls,
  onToggle,
}: {
  shown: boolean
  /** The id of the column it shows. */
  controls: string
  onToggle: () => void
}) {
  return (
    <IconButton
      icon="chatContext"
      label={shown ? "Hide Chat Context" : "Chat Context"}
      draggable={false}
      aria-pressed={shown}
      aria-controls={controls}
      data-chat-context-button
      onClick={(event) => {
        event.stopPropagation()
        onToggle()
      }}
    />
  )
}
