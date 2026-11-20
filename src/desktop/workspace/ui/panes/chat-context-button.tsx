import { IconButton } from "../../../ui/icon-button"
import { useAccessoryOpen } from "../../adapters/store/widget-hosts"
import { chatContextTab } from "../../model/chat-context"

/**
 * Chat context's button in a session pane's header: everything about this
 * chat, opened in front in its right panel (`chat-context.ts`). The panel
 * shows when it was hidden.
 */
export function ChatContextButton({ sessionId }: { sessionId: string }) {
  const open = useAccessoryOpen(sessionId)
  return (
    <IconButton
      icon="chatContext"
      label="Chat Context"
      draggable={false}
      data-chat-context-button
      onClick={(event) => {
        event.stopPropagation()
        open(chatContextTab(sessionId), "panel")
      }}
    />
  )
}
