import type { Activity, Message as MessageValue } from "../../model/transcript"
import { LiveRow } from "./live-row"
import { Message } from "./message"
import "./transcript.css"

/**
 * A conversation the window shows but does not hold — a subagent's — read
 * with the same parts as any chat: the person's bubbles, the agent's prose
 * and steps, and what it is doing now.
 */
export function ReadOnlyTranscript({
  sessionId,
  messages,
  activity,
}: {
  sessionId: string
  messages: readonly MessageValue[]
  activity: Activity | null
}) {
  return (
    <div className="workspace-transcript-inner" data-read-only>
      {messages.map((message) => (
        <Message key={message.id} sessionId={sessionId} message={message} isNew={false} />
      ))}
      {activity ? <LiveRow activity={activity} /> : null}
    </div>
  )
}
