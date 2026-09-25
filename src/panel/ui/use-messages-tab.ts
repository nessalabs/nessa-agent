import * as React from "react"

import { messagesAfter, type MessagesTabState } from "../application/messages-tab"

/**
 * The Messages tab's state, moved only by `messagesAfter`, and what leaving it
 * means: when the tab stops being the one on screen — a real transition, not a
 * remount, which development mode also performs — `onLeave` runs, so what the
 * list said about an earlier archive or delete is let go of then.
 */
export function useMessagesTab(onLeave: () => void) {
  const [messages, setMessages] = React.useState<MessagesTabState>("closed")
  const leave = React.useEffectEvent(onLeave)
  const shown = React.useRef(messages)
  React.useEffect(() => {
    if (shown.current === "viewing" && messages !== "viewing") leave()
    shown.current = messages
  }, [messages])
  const move = React.useCallback(
    (event: Parameters<typeof messagesAfter>[1]) =>
      setMessages((state) => messagesAfter(state, event)),
    [],
  )
  return { messages, move }
}
