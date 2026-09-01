import { useLayoutEffect } from "react"

import { flushCompositor } from "../../host"

export function useFlushOnTurn(enabled: boolean, token: string) {
  useLayoutEffect(() => {
    if (!enabled) return
    void flushCompositor()
  }, [enabled, token])
}
