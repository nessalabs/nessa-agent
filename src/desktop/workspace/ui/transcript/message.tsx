import { memo } from "react"
import { discardUnsent, resendMessage } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import { useInlineWidgetHost } from "../../adapters/store/widget-hosts"
import { InlineWidget, type WidgetRef } from "../../../widgets"
import { groupSteps, type Message as MessageValue } from "../../model/transcript"
import { RichText } from "./rich-text"
import { ToolSteps } from "./tool-steps"
import type { WorkspaceFailureReason } from "../../model/failure"
import { failureCopy } from "../failure-copy"

/**
 * One message. The person's is a bubble, saying so when it has not reached
 * the agent; the agent's is prose, steps, code, lists and widgets, each a
 * card its plugin draws (`InlineWidget`). Memoised on the
 * message itself, so a reply streaming in renders only the message it grows.
 */
export const Message = memo(function Message({
  sessionId,
  message,
  isNew,
}: {
  sessionId: string
  message: MessageValue
  /** Arrived while the conversation was open, so it rises into place. */
  isNew: boolean
}) {
  if (message.role === "user") {
    const failed = message.delivery?.state === "failed" ? message.delivery.reason : null
    return (
      <div
        className="workspace-message"
        data-role="user"
        data-new={isNew || undefined}
        data-sending={message.delivery?.state === "sending" || undefined}
      >
        <div className="workspace-bubble">
          {message.parts.map((part, index) =>
            part.kind === "text" ? (
              <p key={index}>
                <RichText text={part.text} />
              </p>
            ) : null,
          )}
        </div>
        {failed ? (
          <Unsent sessionId={sessionId} messageId={message.id} reason={failed} />
        ) : null}
      </div>
    )
  }
  const groups = groupSteps(message.parts)
  // A reply that has only just begun streaming has nothing to show yet.
  if (
    groups.every((group) => !Array.isArray(group) && group.kind === "text" && !group.text)
  )
    return null
  return (
    <div className="workspace-message" data-role="agent" data-new={isNew || undefined}>
      <div className="workspace-message-body">
        {groups.map((group, index) => {
          if (Array.isArray(group)) return <ToolSteps key={index} steps={group} />
          if (group.kind === "text")
            return group.text ? (
              <p key={index}>
                <RichText text={group.text} />
              </p>
            ) : null
          if (group.kind === "code")
            return (
              <pre key={index} className="workspace-code">
                <code>{group.code}</code>
              </pre>
            )
          if (group.kind === "widget")
            return (
              <MessageWidget key={index} sessionId={sessionId} widget={group.widget} />
            )
          if (group.kind === "list")
            return (
              <ul key={index} className="workspace-list-items">
                {group.items.map((item, row) => (
                  <li key={row}>
                    <RichText text={item} />
                  </li>
                ))}
              </ul>
            )
          return null
        })}
      </div>
    </div>
  )
})

/** A widget in a message: its card, whose pane opens beside this conversation. */
function MessageWidget({ sessionId, widget }: { sessionId: string; widget: WidgetRef }) {
  const host = useInlineWidgetHost(widget, sessionId)
  return <InlineWidget widget={widget} host={host} />
}

/** Why a message did not reach the agent, and the two ways on: send it again, or let it go. */
function Unsent({
  sessionId,
  messageId,
  reason,
}: {
  sessionId: string
  messageId: string
  reason: WorkspaceFailureReason
}) {
  const dispatch = useWorkspaceDispatch()
  return (
    <div className="workspace-message-failed" role="status">
      <span>Not sent. {failureCopy(reason)}</span>
      <button
        type="button"
        className="workspace-link-button"
        onClick={() =>
          void dispatch(resendMessage({ sessionId, messageId, initiator: "person" }))
        }
      >
        Send Again
      </button>
      <button
        type="button"
        className="workspace-link-button"
        onClick={() => dispatch(discardUnsent({ sessionId, messageId }))}
      >
        Discard
      </button>
    </div>
  )
}
