import { memo } from "react"
import { TranscriptDivider } from "@nessa-ui/react/transcript-divider"
import {
  PublishedFiles,
  MessageImages,
  TranscriptNote,
  UnreadableOverflowLine,
  UnreadablePartLine,
} from "../../../../conversation"
import { useInlineWidgetHost } from "../../adapters/store/widget-hosts"
import { InlineWidget, type WidgetRef } from "../../../widgets"
import { handoffLine } from "../../model/handoff"
import { paneItemKey, widgetItem } from "../../model/pane-item"
import {
  groupSteps,
  terminalLabel,
  type Message as MessageValue,
  type MessageApp,
  type TurnTerminal,
} from "../../model/transcript"
import { reportVerb, threadReportOf } from "../../model/thread-report"
import { ChatPill } from "./chat-pill"
import { AgentProse, RichText } from "./rich-text"
import { named, Saying, type Said } from "./said"
import { ToolSteps } from "./tool-steps"

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
 * (`UnreadablePartLine`), and so is the point where the agent ran on in a
 * new session, a switch or a fork (`handoffLine`). Memoised on the
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
  if (message.role === "handoff") {
    const line = handoffLine(message.handoff)
    return (
      <div
        className="workspace-message"
        data-role="handoff"
        data-handoff={message.handoff.kind}
        data-new={isNew || undefined}
      >
        <TranscriptDivider className="[overflow-wrap:anywhere]" meta={line.meta}>
          {line.label}
        </TranscriptDivider>
      </div>
    )
  }
  if (message.role === "user") {
    // A thread's report reaches the agent whole; here it is one line: the
    // thread, and what it did (`model/thread-report.ts`).
    const first = message.parts[0]
    const report = first?.kind === "text" ? threadReportOf(first.text) : null
    if (report)
      return (
        <div
          className="workspace-message"
          data-role="thread-report"
          data-new={isNew || undefined}
        >
          <p className="workspace-thread-report" data-outcome={report.outcome}>
            <ChatPill sessionId={report.thread} />
            <span>{reportVerb(report.outcome)}</span>
          </p>
        </div>
      )
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
            part.kind === "text" && part.text ? (
              <p key={index}>
                <RichText text={part.text} />
              </p>
            ) : part.kind === "attachments" ? (
              <MessageImages key={index} content={part.content} />
            ) : null,
          )}
        </div>
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
          if (Array.isArray(group))
            return (
              <ToolSteps key={index} steps={group} settled={message.working !== true} />
            )
          // What the agent said is Markdown, whitespace and all: its line
          // breaks and blank lines are structure, so it is parsed, not set in
          // one paragraph (`AgentProse`).
          if (group.kind === "text")
            return group.text.trim() ? (
              <AgentProse
                key={index}
                text={group.text}
                streaming={message.working}
                fresh={isNew}
              />
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
