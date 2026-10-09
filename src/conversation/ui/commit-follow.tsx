import { useEffect, useRef } from "react"
import type { NessaClient } from "@nessa/client"

import type { ConversationTabs } from "../model"
import { conversationView } from "../adapters/gateway/effects"
import {
  createCommitFollower,
  isCommitSocket,
  viewIsInProgress,
} from "../adapters/gateway/sync-path"
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
 * conversation. Watches use `connect`, a socket of their own, so a watch
 * refusal does not sign this session out. With no binding, or when the watch
 * ends, the timer in `useConversation` keeps reading. A chat that is running
 * or waiting keeps that timer even while the watch is held.
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
  const targets = useRef<string[]>([])
  const tabsRef = useRef(tabs)
  targets.current = watchTargets(tabs)
  tabsRef.current = tabs
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
        openWatchConnection: async () => {
          try {
            return await connect()
          } catch {
            return undefined
          }
        },
        targets: () => targets.current,
        recordWatchTargets: () =>
          targets.current.filter((id) => {
            const tab = tabsRef.current.conversations.find(
              (item) => item.serverConversationId === id,
            )
            return tab !== undefined && tab.phase !== "idle"
          }),
        listMembership: async () => {
          const current = session.get()
          if (!current) return { rows: [], complete: false }
          const [open, archived] = await Promise.all([
            current.conversation.list(),
            current.conversation.list({ archived: true }),
          ])
          const rows = [...open.conversations, ...archived.conversations].map((row) => ({
            conversationId: row.conversationId,
            title: row.title,
            preview: row.preview,
            updatedAtMs: row.updatedAtMs,
            createdAtMs: row.createdAtMs,
            archived: row.archived,
          }))
          return { rows, complete: open.complete && archived.complete }
        },
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
              running: viewIsInProgress(view),
            }),
          )
        },
        onFallback: () => {
          if (!disposed) dispatch(commitFollowSet("poll"))
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
  }, [connect, dispatch, session])

  useEffect(() => {
    follower.current?.retarget()
  }, [tabs])

  return null
}
