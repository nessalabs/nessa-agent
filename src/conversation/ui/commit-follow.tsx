import { useEffect, useRef } from "react"
import type { NessaClient } from "@nessa/client"

import type { ConversationTabs } from "../model"
import { conversationView } from "../adapters/gateway/effects"
import { createCommitFollower, isCommitSocket } from "../adapters/gateway/sync-path"
import {
  catalogueApplied,
  commitFollowSet,
  conversationDeleted,
  runningObserved,
} from "../adapters/store/history"
import { followedView } from "../adapters/store/slice"
import { useConversationDispatch, useConversationSelector } from "../adapters/store/hooks"

/** The open conversation, then any busy one, in watch order. */
function watchTargets(tabs: ConversationTabs): string[] {
  const ids: string[] = []
  const active = tabs.conversations.find((item) => item.id === tabs.activeId)
  if (active?.serverConversationId) ids.push(active.serverConversationId)
  for (const item of tabs.conversations) {
    if (item.phase === "idle" || !item.serverConversationId) continue
    if (!ids.includes(item.serverConversationId)) ids.push(item.serverConversationId)
  }
  return ids
}

/**
 * Follows the gateway's commit pings into the panel's list and the open
 * conversation. With no binding, or when the watch ends, the timer in
 * `useConversation` keeps reading.
 */
export function ConversationFollow({
  session,
}: {
  session: {
    get(): NessaClient | null
    subscribe(listener: () => void): () => void
  }
}) {
  const dispatch = useConversationDispatch()
  const tabs = useConversationSelector((state) => state.conversation)
  const targets = useRef<string[]>([])
  targets.current = watchTargets(tabs)
  const follower = useRef<ReturnType<typeof createCommitFollower> | undefined>(undefined)

  useEffect(() => {
    let disposed = false
    let current = follower.current
    const start = () => {
      current?.stop()
      current = undefined
      follower.current = undefined
      const client = session.get()
      if (!client || !isCommitSocket(client)) return
      const next = createCommitFollower({
        client,
        targets: () => targets.current,
        onCatalogue: (update) => {
          dispatch(
            catalogueApplied({
              reset: update.reset,
              removedIds: update.removedIds,
              rows: update.rows.map((row) => ({
                conversationId: row.conversationId,
                title: row.title,
                preview: row.preview,
                updatedAtMs: row.updatedAtMs,
                running: false,
                archived: row.archived,
              })),
            }),
          )
          for (const id of update.removedIds) dispatch(conversationDeleted(id))
        },
        onView: (id, seen) => {
          const view = conversationView(seen)
          dispatch(followedView({ serverId: id, view }))
          dispatch(
            runningObserved({
              conversationId: id,
              running: view.messages.some(
                (message) => message.status === "running" || message.status === "queued",
              ),
            }),
          )
        },
        onFallback: () => {
          if (!disposed) dispatch(commitFollowSet("poll"))
        },
        onRevoked: () => {
          if (!disposed) dispatch(commitFollowSet("revoked"))
        },
      })
      current = next
      follower.current = next
      void next.start().then((outcome) => {
        if (disposed || follower.current !== next) return
        if (outcome === "sync") dispatch(commitFollowSet("sync"))
      })
    }
    start()
    const unsubscribe = session.subscribe(start)
    return () => {
      disposed = true
      unsubscribe()
      current?.stop()
      follower.current = undefined
    }
  }, [dispatch, session])

  useEffect(() => {
    follower.current?.retarget()
  }, [tabs])

  return null
}
