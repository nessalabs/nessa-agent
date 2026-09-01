import { useDispatch, useSelector, type TypedUseSelectorHook } from "react-redux"

import type { LocalStrip } from "../../application/local-strip"

type ConversationRoot = { conversation: LocalStrip }

export const useConversationDispatch = useDispatch
export const useConversationSelector: TypedUseSelectorHook<ConversationRoot> = useSelector
