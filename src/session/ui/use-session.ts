import { statusLabel } from "../model"
import { useSessionSelector } from "../adapters/store/hooks"

export function useSession() {
  const session = useSessionSelector((state) => state.session)
  return {
    ...session,
    statusLabel: statusLabel(session),
  }
}
