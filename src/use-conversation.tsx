import { conversationInStrip } from "./conversation"
import {
  closeConversation,
  openConversation,
  sendDraft,
  setActiveId,
  setDraft,
  stopGenerating,
  useAppDispatch,
  useAppSelector,
} from "./store"

/**
 * The conversation strip on screen: selectors and the actions the chrome
 * dispatches. Clocks live in `ConversationClocks`.
 */
export function useConversation() {
  const dispatch = useAppDispatch()
  const conversations = useAppSelector((state) => state.conversation.conversations)
  const activeId = useAppSelector((state) => state.conversation.activeId)
  const active = conversationInStrip(conversations, activeId)

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
