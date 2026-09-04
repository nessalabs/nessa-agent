import { useDispatch, useSelector, type TypedUseSelectorHook } from "react-redux"

import type { AppDispatch, RootState } from "../../../store"

export const useConversationDispatch: () => AppDispatch = useDispatch
export const useConversationSelector: TypedUseSelectorHook<RootState> = useSelector
