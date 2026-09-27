import { statusLabel } from "../model"
import { retrySession } from "../adapters/store/slice"
import { useSessionDispatch, useSessionSelector } from "../adapters/store/hooks"

export function useSession() {
  const dispatch = useSessionDispatch()
  const session = useSessionSelector((state) => state.session)
  return {
    ...session,
    retry: () => {
      dispatch(retrySession())
    },
    statusLabel: statusLabel(session),
  }
}
