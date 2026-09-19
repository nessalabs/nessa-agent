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
  isAttachmentPending: (conversationId: string) => boolean,
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

  // A fresh conversation gets a fresh composer — the pane does not follow
  // somebody into a tab they did not open it in.
  React.useEffect(() => setExpanded(false), [chat.active.id])

  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const content = composerRef.current?.getContent()
    if (isAttachmentPending(chat.active.id)) return
    if (chat.active.draft.some((part) => part.type === "file")) return
    if (!content) return
    // The pane closes on the draft having gone, not on this handler having run.
    // Calling `submit` proves nothing: the gateway may be away, the text may be
    // over the size the gateway takes, and in both the draft is still here and
    // still needs somewhere to be read. So the composer's own offer to collapse
    // is declined for every submit (see `takesExpansion`) and this closes it
    // once the message is actually gone.
    void chat.submit(fromEditor(content)).then((taken) => {
      if (taken) setExpanded(false)
    })
  }

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
