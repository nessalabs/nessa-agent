import { useLayoutEffect } from "react"

import { flushCompositor } from "../../host"

/**
 * Layout compositors drop vacated tiles after a turn lands. This is a host
 * subscription, not a product rule — the transcript does not call the host.
 */
export function useFlushOnTurn(enabled: boolean, token: string) {
  useLayoutEffect(() => {
    if (!enabled) return
    void flushCompositor()
  }, [enabled, token])
}
