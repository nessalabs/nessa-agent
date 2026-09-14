import * as React from "react"
import type {
  ChatComposerContent,
  ChatComposerChip,
  ChatComposerEditorHandle,
} from "@nessa-ui/react/chat-composer-editor"
import { fromEditor, pastedTextLabel, type useConversation } from "../../conversation"

/** Own editor lifecycle and pasted-viewer state; App only composes the surfaces. */
export function useComposer(chat: ReturnType<typeof useConversation>) {
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
  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const content = composerRef.current?.getContent()
    if (content) chat.submit(fromEditor(content))
  }
  function changeContent(content: ChatComposerContent) {
    chat.setDraft(fromEditor(content))
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
