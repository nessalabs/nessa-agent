import * as React from "react"

import { ConversationList } from "../../conversation"
import { MESSAGES_TAB_ID } from "../application/messages-tab"

/**
 * The Messages tab's panel: the conversation list, laid straight into the tab
 * as a transcript is. On the clear surface alone it stands on a fill, for
 * readability over the desktop (see `.nessa-messages-pane` in styles.css).
 */
export function MessagesTab(props: React.ComponentProps<typeof ConversationList>) {
  return (
    <div
      className="nessa-messages-tab"
      role="tabpanel"
      id={`chat-tab-panel-${MESSAGES_TAB_ID}`}
      aria-label="Messages"
    >
      <div className="nessa-messages-pane">
        <ConversationList {...props} />
      </div>
    </div>
  )
}
