import {
  createContext,
  useContext,
  useLayoutEffect,
  useRef,
  type MutableRefObject,
  type RefObject,
} from "react"
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

/**
 * Which session's reply pill holds the caret, or is to take it (⌘R), for as
 * long as the overview is open: the overview's, given to every pill in it.
 * A pill drawn anew for that session — its row moved to another group by the
 * reply just sent, or its peek drawn for ⌘R — takes the caret as it mounts,
 * so the keyboard stays with the session, not with the element that was
 * drawn for it. Focus landing anywhere else lets the caret go.
 */
export const ReplyCaret = createContext<MutableRefObject<string | null> | null>(null)

/** The most lines the pill grows to before it scrolls. */
const pillLines = 4

/**
 * Sets the pill's height from its draft. An empty draft is already one line
 * (`rows={1}`), so this clears any inline height and does not read layout
 * (`reply-pill.test.tsx`).
 */
export function sizeReplyPill(textarea: HTMLTextAreaElement, draft: string): void {
  if (draft === "") {
    textarea.style.height = ""
    return
  }
  textarea.style.height = "auto"
  const style = getComputedStyle(textarea)
  const line = Number.parseFloat(style.lineHeight) || 18
  const padding =
    Number.parseFloat(style.paddingTop) + Number.parseFloat(style.paddingBottom)
  textarea.style.height = `${Math.min(textarea.scrollHeight, line * pillLines + padding)}px`
}

/**
 * A reply to a session, written where its peek is, without opening it: one
 * line that grows to four, and the send button — the pane composer's
 * pieces, pared to a pill. Its draft is the session's own
 * (`setComposerText`), so it is the same text the pane's composer shows, and
 * sending is the workspace's `sendMessage`, as from the pane, which empties
 * it. The reply streams into the peek above; the caret stays with the
 * session's pill (`ReplyCaret`), even when sending moves its row and draws
 * the pill anew. Escape gives the keyboard back to the list.
 */
export function ReplyPill({
  sessionId,
  agent,
  fieldRef,
  onLeave,
}: {
  sessionId: string
  /** Who it goes to, as the placeholder says: "Reply to Codex…". */
  agent: string
  fieldRef?: RefObject<HTMLTextAreaElement | null>
  onLeave: () => void
}) {
  const dispatch = useWorkspaceDispatch()
  const draft = useWorkspaceSelector((state) => selectComposerText(state, sessionId))
  const sending = useWorkspaceSelector((state) => selectSending(state, sessionId))
  const unsent = useWorkspaceSelector((state) => selectUnsent(state, sessionId))
  const own = useRef<HTMLTextAreaElement>(null)
  const field = fieldRef ?? own
  const caret = useContext(ReplyCaret)

  // Drawn for the session the caret belongs with: the caret comes here,
  // before the frame paints, so the next key typed lands in the pill.
  useLayoutEffect(() => {
    const textarea = field.current
    if (textarea && caret?.current === sessionId && document.activeElement !== textarea)
      textarea.focus()
  }, [caret, sessionId, field])

  // Grows with the draft, before paint, up to four lines; then it scrolls.
  useLayoutEffect(() => {
    const textarea = field.current
    if (!textarea) return
    sizeReplyPill(textarea, draft)
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
          onFocus={() => {
            if (caret) caret.current = sessionId
          }}
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
      ) : null}
    </div>
  )
}
