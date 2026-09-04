import { useDispatch, useSelector, type TypedUseSelectorHook } from "react-redux"

import type { LocalTabs } from "../../application/local-tabs"

type ConversationRoot = { conversation: LocalTabs }

export const useConversationDispatch = useDispatch
export const useConversationSelector: TypedUseSelectorHook<ConversationRoot> = useSelector
