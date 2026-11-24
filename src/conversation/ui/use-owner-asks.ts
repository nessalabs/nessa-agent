import { useEffect, useMemo, useRef, useState } from "react"
import { followOwnerAsks, type WaitingAsk } from "../adapters/store/asks"
import { useConversationDispatch, useConversationSelector } from "../adapters/store/hooks"
import { openAsked } from "../adapters/store/slice"
import { canUseGateway } from "../../session"

/**
 * The decisions agents asked the person, by conversation, and a tab for each.
 *
 * Each new ask opens its conversation as a tab at the end of the strip,
 * without showing it: what the person is looking at, and where they are
 * typing, stays theirs. An ask opens its tab once; closed, it stays closed
 * until the agent asks again.
 */
export function useOwnerAsks(): ReadonlyMap<string, WaitingAsk> {
  const dispatch = useConversationDispatch()
  const gatewayAvailable = useConversationSelector((state) =>
    canUseGateway(state.session),
  )
  const [asks, setAsks] = useState<WaitingAsk[]>([])
  useEffect(() => {
    if (!gatewayAvailable) return
    return dispatch(followOwnerAsks(setAsks))
  }, [dispatch, gatewayAvailable])
  const opened = useRef(new Set<string>())
  useEffect(() => {
    for (const waiting of asks) {
      if (opened.current.has(waiting.ask.askId)) continue
      opened.current.add(waiting.ask.askId)
      dispatch(
        openAsked({
          serverConversationId: waiting.serverConversationId,
          title: waiting.title,
        }),
      )
    }
  }, [dispatch, asks])
  return useMemo(
    () => new Map(asks.map((waiting) => [waiting.serverConversationId, waiting])),
    [asks],
  )
}
