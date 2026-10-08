/**
 * What a conversation's pane header shows of its subagents. Nothing when
 * the preview is off, or when the source has no ready children; otherwise
 * the faces, the busiest first (`byActivity`). `subagent-stack.test.tsx`.
 */
import { useSyncExternalStore } from "react"
import type { AvatarStackItem } from "@nessa-ui/react/avatar-stack"
import { useSubagentsPreview } from "../../adapters/window-preferences"
import { plural } from "../../model/counts"
import { useSubagentSource } from "../adapters/react/source-context"
import { byActivity } from "../model/subagent"
import { stackItem } from "./stack-item"

export function useSubagentStack(sessionId: string): {
  readonly items: readonly AvatarStackItem[]
  readonly label: string
} | null {
  const [preview] = useSubagentsPreview()
  const source = useSubagentSource()
  const read = useSyncExternalStore(
    (listener) => source.subscribe(listener),
    () => source.forSession(sessionId),
  )
  if (preview === "off" || read.kind !== "ready" || read.subagents.length === 0)
    return null
  const ordered = byActivity(read.subagents)
  return { items: ordered.map(stackItem), label: plural(ordered.length, "agent") }
}
