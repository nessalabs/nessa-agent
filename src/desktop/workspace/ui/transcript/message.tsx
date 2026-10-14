import { memo } from "react"
import {
  PublishedFiles,
  TranscriptNote,
  UnreadableOverflowLine,
  UnreadablePartLine,
} from "../../../../conversation"
import { discardUnsent, resendMessage } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import { useInlineWidgetHost } from "../../adapters/store/widget-hosts"
import { InlineWidget, type WidgetRef } from "../../../widgets"
import { paneItemKey, widgetItem } from "../../model/pane-item"
import {
  groupSteps,
  terminalLabel,
  type Message as MessageValue,
  type MessageApp,
  type TurnTerminal,
} from "../../model/transcript"
import { RichText } from "./rich-text"
import { named, Saying, type Said } from "./said"
import { ToolSteps } from "./tool-steps"
import type { WorkspaceFailureReason } from "../../model/failure"
import { failureCopy } from "../failure-copy"

/**
 * Who wrote a message of the person's on their behalf: the app's tool, from
 * its server, each name isolated from the words around it (`said.tsx`).
 */
export function messageAuthor(app: MessageApp): Said {
  return ["Sent by ", named(app.tool), ", from ", named(app.server)]
}

/**
 * One message. The person's is a bubble, saying so when it has not reached
 * the agent, and saying above it which MCP App wrote it when one did
 * (`messageAuthor`, #390); the agent's is prose, steps, code, lists,
 * widgets, each a card its plugin draws (`InlineWidget`), and the files it
 * published, each a card the conversation vertical draws and reads back
 * (`PublishedFiles`). A note somebody left is a row of its own, neither
 * (`TranscriptNote`). A record this read could not apply is one line
 * (`UnreadablePartLine`). Memoised on the
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
  if (message.role === "note")
    return (
      <div className="workspace-message" data-role="note" data-new={isNew || undefined}>
        <TranscriptNote note={message.note} />
      </div>
    )
  if (message.role === "unreadable")
    return (
      <div
        className="workspace-message"
        data-role="unreadable"
        data-new={isNew || undefined}
      >
        <UnreadablePartLine part={message.unreadable} />
      </div>
    )
  if (message.role === "unreadable-overflow")
    return (
      <div
        className="workspace-message"
        data-role="unreadable-overflow"
        data-new={isNew || undefined}
      >
        <UnreadableOverflowLine />
      </div>
    )
  if (message.role === "user") {
    const failed = message.delivery?.state === "failed" ? message.delivery.reason : null
    return (
      <div
        className="workspace-message"
        data-role="user"
        data-new={isNew || undefined}
        data-sending={message.delivery?.state === "sending" || undefined}
      >
        {message.app ? (
          <p
            className="workspace-message-author"
            data-message-app={`${message.app.server}/${message.app.tool}`}
          >
            <Saying said={messageAuthor(message.app)} />
          </p>
        ) : null}
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
        {message.terminal ? <TurnTerminalMark terminal={message.terminal} /> : null}
      </div>
    )
  }
  const groups = groupSteps(message.parts)
  const widgetOccurrences = new Map<string, number>()
  // A reply that has only just begun streaming has nothing to show yet.
  // A turn that ended still shows how it ended, even when it said nothing.
  if (
    groups.every(
      (group) => !Array.isArray(group) && group.kind === "text" && !group.text,
    ) &&
    !message.terminal
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
          if (group.kind === "widget") {
            const identity = paneItemKey(widgetItem(group.widget))
            const occurrence = widgetOccurrences.get(identity) ?? 0
            widgetOccurrences.set(identity, occurrence + 1)
            return (
              // Each occurrence keeps its view when surrounding message parts change.
              <MessageWidget
                key={JSON.stringify([identity, occurrence])}
                sessionId={sessionId}
                widget={group.widget}
              />
            )
          }
          if (group.kind === "files")
            return (
              <PublishedFiles
                key={index}
                conversationId={sessionId}
                files={group.files}
              />
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
      {message.terminal ? <TurnTerminalMark terminal={message.terminal} /> : null}
    </div>
  )
})

/**
 * How a turn ended, in the gateway's words: the status, and its diagnostic
 * when it sent one. Separate from a message this window could not deliver.
 */
function TurnTerminalMark({ terminal }: { terminal: TurnTerminal }) {
  return (
    <div
      className="workspace-turn-terminal"
      role="status"
      data-turn-terminal={terminal.status}
      {...(terminal.error === undefined ? {} : { "data-turn-error": terminal.error })}
    >
      <span className="workspace-turn-terminal-status">
        {terminalLabel(terminal.status)}
      </span>
      {terminal.error === undefined ? null : (
        <span className="workspace-turn-terminal-error">{terminal.error}</span>
      )}
    </div>
  )
}

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
