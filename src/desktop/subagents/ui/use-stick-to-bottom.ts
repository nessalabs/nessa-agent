/**
 * Keeps a scroller on its latest line while the reader is already there.
 * Scrolled up, a growing transcript leaves them where they are
 * (`use-stick-to-bottom.test.tsx`).
 */
import { useLayoutEffect, useRef } from "react"

/** How close to the end still counts as reading the latest line. */
export const stickWithin = 40

/**
 * `identity` is whose transcript this is: a different one starts pinned.
 * `signature` is what grew. While pinned, the scroller moves to the end.
 */
export function useStickToBottom(identity: string, signature: string) {
  const ref = useRef<HTMLDivElement | null>(null)
  const pinned = useRef(true)
  const seen = useRef(identity)

  const onScroll = () => {
    const node = ref.current
    if (!node) return
    pinned.current = node.scrollHeight - node.scrollTop - node.clientHeight < stickWithin
  }

  useLayoutEffect(() => {
    if (seen.current !== identity) {
      seen.current = identity
      pinned.current = true
    }
    const node = ref.current
    if (!node || !pinned.current) return
    node.scrollTop = node.scrollHeight
  }, [identity, signature])

  return { ref, onScroll }
}
