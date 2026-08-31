import { activeConversation } from "../application/queries/active-conversation"
import {
  closeConversation,
  openConversation,
  sendDraft,
  setActiveId,
  setDraft,
  stopGenerating,
} from "../adapters/store/slice"
import { useConversationDispatch, useConversationSelector } from "../adapters/store/hooks"

/**
 * Binds the strip on screen to the store. No product rules: it reads the
 * projection and dispatches the named commands.
 */
export function useConversation() {
  const dispatch = useConversationDispatch()
  const strip = useConversationSelector((state) => state.conversation)
  const conversations = strip.conversations
  const active = activeConversation(strip)

  return {
    conversations,
    active,
    setActiveId: (id: string) => dispatch(setActiveId(id)),
    submit: () => dispatch(sendDraft()),
    openConversation: () => {
      dispatch(openConversation())
    },
    closeConversation: (id: string) => dispatch(closeConversation(id)),
    setDraft: (draft: string) => dispatch(setDraft({ draft })),
    stopGenerating: () => dispatch(stopGenerating()),
  }
}
