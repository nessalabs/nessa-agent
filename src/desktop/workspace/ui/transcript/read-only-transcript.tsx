/**
 * A conversation with nothing to send: the workspace's own message and live
 * row, and no composer. A subagent's transcript is this
 * (`subagents/ui/subagents-panel.tsx`).
 */
import type { Activity, Message as MessageValue } from "../../model/transcript"
import { LiveRow } from "./live-row"
import { Message } from "./message"
import "./transcript.css"

export function ReadOnlyTranscript({
  messages,
  activity,
}: {
  messages: readonly MessageValue[]
  activity: Activity | null
}) {
  return (
    <div className="workspace-transcript" data-read-only>
      <div className="workspace-transcript-inner">
        {messages.map((message) => (
          <Message key={message.id} sessionId="" message={message} isNew={false} />
        ))}
        {activity ? <LiveRow activity={activity} /> : null}
      </div>
    </div>
  )
}
