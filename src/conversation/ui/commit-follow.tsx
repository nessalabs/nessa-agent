import { useEffect, useRef } from "react"
import type { NessaClient } from "@nessa/client"

import type { ConversationTabs } from "../model"
import { createChangeFollower, type ChangeFollower } from "../adapters/gateway/sync-path"
import { listConversations, recordFollowSet } from "../adapters/store/history"
import { refreshConversation } from "../adapters/store/slice"
import { useConversationDispatch, useConversationSelector } from "../adapters/store/hooks"

/**
 * The open chat, when its poll can stop. Only a settled idle chat can take
 * the one record slot. A turn still starting, thinking, or streaming is the
 * live view, and a commit ping does not carry that text.
 */
function recordTarget(tabs: ConversationTabs): string[] {
  const active = tabs.conversations.find((item) => item.id === tabs.activeId)
  if (!active?.serverConversationId) return []
  if (active.phase !== "idle") return []
  if ((active.remote?.permissions.length ?? 0) > 0) return []
  if ((active.remote?.questions.length ?? 0) > 0) return []
  if (active.remote?.lifecycle.phase === "starting") return []
  return [active.serverConversationId]
}

function tabFor(tabs: ConversationTabs, serverId: string) {
  return tabs.conversations.find((item) => item.serverConversationId === serverId)
}

const FOLLOW_CAP_MS = 60_000

/**
 * Follows commit pings into the panel's existing list and read. Watches use
 * `connect`, a socket of their own, so a watch refusal does not sign this
 * session out. The list watch asks `listConversations`. A record watch asks
 * `refreshConversation` for that chat and is the only chat whose poll stops.
 * An access refusal stays on that poll. A dropped socket tries again, waiting
 * longer each time up to a minute. Reconnecting suspends the record watch
 * until the socket registers it again.
 */
export function ConversationFollow({
  session,
  connect,
}: {
  session: {
    get(): NessaClient | null
    subscribe(listener: () => void): () => void
  }
  /** A new gateway connection used only for watches. */
  connect: () => Promise<NessaClient>
}) {
  const dispatch = useConversationDispatch()
  const tabs = useConversationSelector((state) => state.conversation)
  const tabsRef = useRef(tabs)
  tabsRef.current = tabs
  const follower = useRef<ChangeFollower | undefined>(undefined)

  useEffect(() => {
    let disposed = false
    let current: ChangeFollower | undefined
    let retryTimer: ReturnType<typeof setTimeout> | undefined
    let gaveUp = false
    let delay = 1_000
    const clearRetry = () => {
      if (retryTimer !== undefined) clearTimeout(retryTimer)
      retryTimer = undefined
    }
    const later = (run: () => void) => {
      clearRetry()
      delay = Math.min(delay * 2, FOLLOW_CAP_MS)
      retryTimer = setTimeout(() => {
        retryTimer = undefined
        run()
      }, delay)
    }
    const start = () => {
      clearRetry()
      // stop() does not run onFallback, so a session swap must not leave the
      // previous chat's record watch gating the poll.
      dispatch(recordFollowSet(null))
      current?.stop()
      current = undefined
      follower.current = undefined
      if (disposed || gaveUp || !session.get()) return
      const next = createChangeFollower({
        openConnection: async () => {
          const client = await connect()
          return {
            watches: {
              records: ({ conversationId }) =>
                client.watches.ownedRecords(conversationId),
              catalogue: () => client.watches.ownedCatalogue(),
              unwatch: (watchId) => client.watches.unwatch(watchId),
            },
            on: (event, handler) => client.on(event, handler),
            onConnectionStateChange: (handler) => client.onConnectionStateChange(handler),
            close: () => client.close(),
          }
        },
        recordTargets: () => recordTarget(tabsRef.current),
        onListChanged: () => {
          if (!disposed) void dispatch(listConversations())
        },
        onConversationChanged: (serverId) => {
          const tab = tabFor(tabsRef.current, serverId)
          if (tab && !disposed) void dispatch(refreshConversation(tab.id))
        },
        onRecordHeld: (serverId) => {
          if (disposed) return
          dispatch(recordFollowSet(serverId ?? null))
          if (!serverId) return
          const tab = tabFor(tabsRef.current, serverId)
          if (tab) void dispatch(refreshConversation(tab.id))
        },
        onWatching: (held) => {
          if (disposed) return
          if (!held) {
            dispatch(recordFollowSet(null))
            return
          }
          void dispatch(listConversations())
        },
        onFallback: (reason) => {
          if (disposed) return
          dispatch(recordFollowSet(null))
          current = undefined
          follower.current = undefined
          if (reason === "watch-refused") {
            gaveUp = true
            return
          }
          later(start)
        },
      })
      current = next
      follower.current = next
      void next.start().then((outcome) => {
        if (disposed || follower.current !== next) return
        if (outcome === "sync") {
          delay = 1_000
          return
        }
        follower.current = undefined
        current = undefined
        dispatch(recordFollowSet(null))
        if (outcome === "refused") {
          gaveUp = true
          return
        }
        later(start)
      })
    }
    start()
    const unsubscribe = session.subscribe(() => {
      gaveUp = false
      delay = 1_000
      start()
    })
    return () => {
      disposed = true
      clearRetry()
      unsubscribe()
      current?.stop()
      follower.current = undefined
      dispatch(recordFollowSet(null))
    }
  }, [connect, dispatch, session])

  useEffect(() => {
    follower.current?.retarget()
  }, [tabs])

  return null
}
