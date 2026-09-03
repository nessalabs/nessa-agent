import { useDispatch, useSelector, type TypedUseSelectorHook } from "react-redux"

import type { SessionState } from "../../model"

type SessionRoot = { session: SessionState }

export const useSessionDispatch = useDispatch
export const useSessionSelector: TypedUseSelectorHook<SessionRoot> = useSelector
