/**
 * Keeps a scroller on its latest line while the reader is already there.
 * Scrolled up, a growing transcript leaves them where they are.
 * A tool disclosure holds that follow until its motion ends
 * (`use-stick-to-bottom.test.tsx`, `tool-disclosure.ts`).
 */
import { useLayoutEffect, useRef } from "react"
import {
  followTranscriptEnd,
  watchToolDisclosure,
} from "../../workspace/ui/transcript/tool-disclosure"

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
    const node = ref.current
    if (!node) return
    return watchToolDisclosure(node, () => {
      pinned.current =
        node.scrollHeight - node.scrollTop - node.clientHeight < stickWithin
    })
  }, [identity])

  useLayoutEffect(() => {
    if (seen.current !== identity) {
      seen.current = identity
      pinned.current = true
    }
    const node = ref.current
    if (!node) return
    followTranscriptEnd(node, pinned.current)
  }, [identity, signature])

  return { ref, onScroll }
}
