import { useDispatch, useSelector, type TypedUseSelectorHook } from "react-redux"

import type { ConversationStrip } from "../../model"

type ConversationRoot = { conversation: ConversationStrip }

export const useConversationDispatch = useDispatch
export const useConversationSelector: TypedUseSelectorHook<ConversationRoot> = useSelector
