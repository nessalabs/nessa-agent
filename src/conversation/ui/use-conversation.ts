import {
  contentText,
  hasFileAttachments,
  type FileAttachment,
  type MessageContent,
} from "../model"
import { activeConversation } from "../application/queries/active-conversation"
import {
  attachFiles,
  removeFile,
  closeConversation,
  openConversation,
  sendDraft,
  setActive,
  moveActive,
  setDraft,
  stopGenerating,
} from "../adapters/store/slice"
import { useConversationDispatch, useConversationSelector } from "../adapters/store/hooks"

export function useConversation() {
  const dispatch = useConversationDispatch()
  const tabs = useConversationSelector((state) => state.conversation)
  const conversations = tabs.conversations
  const active = activeConversation(tabs)

  return {
    attachFiles: (files: FileAttachment[], conversationId: string) =>
      dispatch(attachFiles({ files, conversationId })),
    removeFile: (id: string) => dispatch(removeFile(id)),
    conversations,
    active,
    moveActive: (direction: -1 | 1) => dispatch(moveActive(direction)),
    setActive: (id: string) => dispatch(setActive(id)),
    submit: (content: MessageContent) => {
      const text = contentText(content).trim()
      if (
        !text ||
        active.phase !== "idle" ||
        hasFileAttachments(content) ||
        hasFileAttachments(active.draft)
      )
        return
      void dispatch(sendDraft({ content, id: active.id }))
    },
    openConversation: () => {
      dispatch(openConversation())
    },
    closeConversation: (id: string) => dispatch(closeConversation(id)),
    setDraft: (draft: MessageContent) => dispatch(setDraft({ draft })),
    stopGenerating: () => dispatch(stopGenerating()),
  }
}
