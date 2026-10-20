import { useLayoutEffect, type RefObject } from "react"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectContentKind } from "../../adapters/store/selectors"

/** Sets the workspace's `data-content` from its own subscription (`layouts.test.tsx`). */
export function ContentMark({ root }: { root: RefObject<HTMLElement | null> }) {
  const contentShown = useWorkspaceSelector(selectContentKind)
  useLayoutEffect(() => {
    root.current?.setAttribute("data-content", contentShown)
  }, [contentShown, root])
  return null
}
