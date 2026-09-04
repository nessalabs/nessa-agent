import { activeConversation } from "../application/queries/active-conversation"
import {
  closeConversation,
  openConversation,
  sendDraft,
  setActive,
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
    conversations,
    active,
    setActive: (id: string) => dispatch(setActive(id)),
    submit: () => {
      const text = active.draft.trim()
      if (!text) return
      void dispatch(sendDraft({ text, id: active.id }))
    },
    openConversation: () => {
      dispatch(openConversation())
    },
    closeConversation: (id: string) => dispatch(closeConversation(id)),
    setDraft: (draft: string) => dispatch(setDraft({ draft })),
    stopGenerating: () => dispatch(stopGenerating()),
  }
}
