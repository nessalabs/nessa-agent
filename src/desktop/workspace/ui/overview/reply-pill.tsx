import { useLayoutEffect, useRef, type RefObject } from "react"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { sendMessage, setComposerText } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectComposerText,
  selectSending,
  selectUnsent,
} from "../../adapters/store/selectors"
import { failureCopy } from "../failure-copy"

/** The most lines the pill grows to before it scrolls. */
const pillLines = 4

/**
 * A reply to a session, written where its peek is, without opening it: one
 * line that grows to four, and the send button — the pane composer's
 * pieces, pared to a pill. Its draft is the session's own
 * (`setComposerText`), so it is the same text the pane's composer shows, and
 * sending is the workspace's `sendMessage`, as from the pane, which empties
 * it. The reply streams into the peek above; the pill keeps the caret. Escape
 * gives the keyboard back to the list.
 */
export function ReplyPill({
  sessionId,
  agent,
  note,
  fieldRef,
  onLeave,
}: {
  sessionId: string
  /** Who it goes to, as the placeholder says: "Reply to Codex…". */
  agent: string
  /** A line under the pill, where replying does something the person should know. */
  note?: string
  fieldRef?: RefObject<HTMLTextAreaElement | null>
  onLeave: () => void
}) {
  const dispatch = useWorkspaceDispatch()
  const draft = useWorkspaceSelector((state) => selectComposerText(state, sessionId))
  const sending = useWorkspaceSelector((state) => selectSending(state, sessionId))
  const unsent = useWorkspaceSelector((state) => selectUnsent(state, sessionId))
  const own = useRef<HTMLTextAreaElement>(null)
  const field = fieldRef ?? own

  // Grows with the draft, before paint, up to four lines; then it scrolls.
  useLayoutEffect(() => {
    const textarea = field.current
    if (!textarea) return
    textarea.style.height = "auto"
    const style = getComputedStyle(textarea)
    const line = Number.parseFloat(style.lineHeight) || 18
    const padding =
      Number.parseFloat(style.paddingTop) + Number.parseFloat(style.paddingBottom)
    textarea.style.height = `${Math.min(textarea.scrollHeight, line * pillLines + padding)}px`
  }, [draft, field])

  // Sent, the text goes by the workspace's own rule (`sendMessage`), as from the pane.
  const send = () => {
    if (draft.trim() === "") return
    void dispatch(sendMessage({ sessionId, text: draft, initiator: "person" }))
  }

  return (
    <div className="agents-reply" data-reply-for={sessionId}>
      <form
        className="agents-reply-pill"
        data-sending={sending || undefined}
        onSubmit={(event) => {
          event.preventDefault()
          send()
        }}
      >
        <textarea
          ref={field}
          rows={1}
          aria-label={`Reply to ${agent}`}
          placeholder={`Reply to ${agent}…`}
          value={draft}
          onChange={(event) =>
            dispatch(setComposerText({ sessionId, text: event.target.value }))
          }
          onKeyDown={(event) => {
            // The list's keys stay out of the field.
            event.stopPropagation()
            if (event.key === "Escape") {
              event.preventDefault()
              onLeave()
              return
            }
            // Return sends; Shift-Return, and Return while composing a word, do not.
            if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing)
              return
            event.preventDefault()
            send()
          }}
        />
        <button
          type="submit"
          className="desktop-send agents-reply-send"
          aria-label="Send"
          disabled={draft.trim() === ""}
          {...tooltip("Send", { shortcut: "↩" })}
        >
          <DesktopIcon name="send" />
        </button>
      </form>
      {unsent ? (
        <p className="agents-reply-note" data-tone="failure" role="status">
          Not sent. {failureCopy(unsent)} Open the session to send it again.
        </p>
      ) : sending ? (
        <p className="agents-reply-note" role="status">
          Sending…
        </p>
      ) : note ? (
        <p className="agents-reply-note">{note}</p>
      ) : null}
    </div>
  )
}
