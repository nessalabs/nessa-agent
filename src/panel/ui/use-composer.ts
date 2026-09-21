import * as React from "react"
import type {
  ChatComposerContent,
  ChatComposerChip,
  ChatComposerEditorHandle,
} from "@nessa-ui/react/chat-composer-editor"
import { fromEditor, pastedTextLabel, type useConversation } from "../../conversation"
import type { PillComposerExpansionReason } from "@nessa-ui/react/pill-composer"
import { takesExpansion } from "../application/composer-expansion"

/** Own editor lifecycle and pasted-viewer state; App only composes the surfaces. */
export function useComposer(
  chat: ReturnType<typeof useConversation>,
  /** Says so in the panel, and answers true, while an attachment is still being read. */
  declinesPendingAttachment: (conversationId: string) => boolean,
  /**
   * A draft has actually left this composer for the gateway. Only then: a
   * submission the panel or the conversation turned away leaves everything
   * where it was, including whatever the panel was already saying about it.
   */
  onDraftSent: (conversationId: string) => void,
) {
  const composerRef = React.useRef<ChatComposerEditorHandle>(null)
  const setComposerRef = React.useCallback((editor: ChatComposerEditorHandle | null) => {
    composerRef.current = editor
    editor?.focus()
  }, [])
  const focusComposer = React.useCallback(() => composerRef.current?.focus(), [])
  const [viewedPaste, setViewedPaste] = React.useState<{
    conversationId: string
    text: string
  } | null>(null)
  const closePaste = React.useCallback(() => setViewedPaste(null), [])
  const openPaste = React.useCallback(
    (text: string) => setViewedPaste({ conversationId: chat.active.id, text }),
    [chat.active.id],
  )
  // Whether the full-pane editor is open. Owned here rather than left to the
  // composer because only this side knows whether a submit actually sent
  // anything: the rules below turn some away, and a draft that never left is
  // the one thing worth keeping the pane open for.
  const [expanded, setExpanded] = React.useState(false)

  // Whether a submit is waiting for its draft to leave. See `submit`.
  const awaitingDraft = React.useRef(false)
  // A fresh conversation gets a fresh composer — the pane does not follow
  // somebody into a tab they did not open it in.
  React.useEffect(() => setExpanded(false), [chat.active.id])

  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    // The editor's own content when it is there. When it is not — a submit
    // raised while the editor is between mounts — the draft is the same prose,
    // kept in step by `changeContent`, so the submit still goes to `sendDraft`
    // and still ends in a message sent or a reason shown. Returning here instead
    // was a send that did nothing and said nothing.
    const editor = composerRef.current?.getContent()
    const content = editor
      ? fromEditor(editor)
      : chat.active.draft.filter(
          (part) => part.type === "text" || part.type === "pasted-text",
        )
    // A submit that goes nowhere still says why. Only a read in flight is
    // declined here, because only this side knows about it; a draft holding
    // files goes on to `sendDraft`, which refuses it with its reason on the
    // conversation. Returning early for those left the panel looking broken.
    if (declinesPendingAttachment(chat.active.id)) return
    // The pane closes on the draft having gone, not on this handler having run.
    // Calling `submit` proves nothing: the gateway may be away, the text may be
    // over the size the gateway takes, and in both the draft is still here and
    // still needs somewhere to be read. So the composer's own offer to collapse
    // is declined for every submit (see `takesExpansion`), and the effect below
    // closes it when the draft is actually gone.
    //
    // Watched rather than awaited. `submit` settles when the gateway has been
    // created, written to and read back, while the draft leaves the composer at
    // the moment the submission starts — so awaiting it held the pane open,
    // empty, for a whole round trip, and for good against a gateway that
    // accepts the connection and then answers nothing.
    awaitingDraft.current = true
    const sending = chat.active.id
    void chat.submit(content).then((taken) => {
      // Turned away: the draft is still here, so nothing is waiting for it to
      // go. Left set, the next unrelated emptying of the draft would close a
      // pane nobody asked to close.
      if (!taken) {
        awaitingDraft.current = false
        return
      }
      onDraftSent(sending)
    })
  }

  // The draft emptying is the submission having started — the one fact that
  // says the message left this composer.
  React.useEffect(() => {
    if (!awaitingDraft.current || chat.active.draft.length > 0) return
    awaitingDraft.current = false
    setExpanded(false)
  }, [chat.active.draft])

  /** Every expansion change the composer proposes, minus the ones we decline. */
  function changeExpanded(next: boolean, reason: PillComposerExpansionReason) {
    if (takesExpansion(next, reason)) setExpanded(next)
  }
  function changeContent(content: ChatComposerContent) {
    chat.setDraft([
      ...fromEditor(content),
      ...chat.active.draft.filter((part) => part.type === "file"),
    ])
  }
  function pressChip(chip: ChatComposerChip) {
    if (chip.textValue !== undefined) openPaste(chip.textValue)
  }
  function pasteAttachment(text: string) {
    composerRef.current?.insertChip({
      id: crypto.randomUUID(),
      kind: "pasted-text",
      label: pastedTextLabel(text),
      textValue: text,
    })
  }
  return {
    expanded,
    changeExpanded,
    composerRef,
    setComposerRef,
    viewedPaste,
    closePaste,
    openPaste,
    focusComposer,
    submit,
    changeContent,
    pressChip,
    pasteAttachment,
  }
}
