/**
 * What the panel shows for one conversation: the source's read, the child
 * it has open, and the scroller that follows that child's transcript.
 * Opening registers a step back; a child that leaves the source is no
 * longer open (`subagents-panel.test.tsx`).
 */
import { useEffect, useLayoutEffect, useRef, useSyncExternalStore } from "react"
import type { WidgetHost } from "../../widgets/ui/plugin"
import { useNow } from "../../workspace/adapters/dom/clock"
import { useSubagentSource } from "../adapters/react/source-context"
import {
  chooseSubagent,
  selectedSubagent,
  subscribeSubagentChoice,
} from "../application/selection"
import { byActivity, summaryLine, type Subagent } from "../model/subagent"
import { useStickToBottom } from "./use-stick-to-bottom"

export function useSubagentsPanel(sessionId: string, host: WidgetHost) {
  const source = useSubagentSource()
  const now = useNow(1000)
  const read = useSyncExternalStore(
    (listener) => source.subscribe(listener),
    () => source.forSession(sessionId),
  )
  const chosenId = useSyncExternalStore(subscribeSubagentChoice, () =>
    selectedSubagent(sessionId),
  )
  const present =
    read.kind === "ready"
      ? (read.subagents.find((subagent) => subagent.id === chosenId) ?? null)
      : null
  const openId = present?.id ?? null

  // A failed or unread read is not the child leaving. Clear only once the
  // source is ready and no longer lists the open child.
  useEffect(() => {
    if (!chosenId || read.kind !== "ready") return
    if (!read.subagents.some((subagent) => subagent.id === chosenId))
      chooseSubagent(sessionId, null)
  }, [chosenId, read, sessionId])

  useEffect(() => {
    if (!openId) return
    return host.onEscape(() => chooseSubagent(sessionId, null))
  }, [openId, host, sessionId])

  const rootRef = useRef<HTMLElement | null>(null)
  useLayoutEffect(() => {
    if (!openId) return
    rootRef.current
      ?.querySelector<HTMLButtonElement>('[data-slot="breadcrumb-link"]')
      ?.focus()
  }, [openId])

  const ordered = read.kind === "ready" ? byActivity(read.subagents) : empty
  const stick = useStickToBottom(
    openId ?? "",
    present
      ? `${present.conversation.messages.length}\0${present.conversation.activity?.label ?? ""}\0${present.conversation.activity?.since ?? ""}`
      : "",
  )

  return {
    rootRef,
    read,
    ordered,
    present,
    summary: summaryLine(ordered),
    now,
    stick,
    choose: (id: string | null) => chooseSubagent(sessionId, id),
  }
}

const empty: readonly Subagent[] = []

export type SubagentsPanelModel = ReturnType<typeof useSubagentsPanel>
