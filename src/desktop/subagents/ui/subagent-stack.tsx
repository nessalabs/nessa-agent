/**
 * A conversation's subagents in its pane header (ADR 329, #332): an
 * `AvatarStack`, the busiest first, given the session's id and `openWidget`
 * alone. A click opens the panel in a pane beside the conversation. Nothing
 * is drawn when the hook has nothing to show.
 */
import { AvatarStack, type AvatarStackItem } from "@nessa-ui/react/avatar-stack"
import { Button } from "@nessa-ui/react/button"
import type { SessionAccessoryProps } from "../../widgets/ui/plugin"
import { subagentsPluginId } from "../application/ports"
import { useSubagentStack } from "./use-subagent-stack"

export function SubagentStack({
  items,
  label,
  onOpen,
}: {
  items: readonly AvatarStackItem[]
  label: string
  onOpen: () => void
}) {
  return (
    <Button
      variant="plain"
      size="28"
      shape="pill"
      data-subagent-stack
      draggable={false}
      onClick={(event) => {
        event.stopPropagation()
        onOpen()
      }}
    >
      <AvatarStack size="sm" label={label} items={items} />
    </Button>
  )
}

/** The plugin's header accessory. The workspace draws it through the slot. */
export function SubagentStackAccessory({ sessionId, openWidget }: SessionAccessoryProps) {
  const stack = useSubagentStack(sessionId)
  if (!stack) return null
  return (
    <SubagentStack
      items={stack.items}
      label={stack.label}
      onOpen={() => openWidget({ plugin: subagentsPluginId, id: sessionId }, "pane")}
    />
  )
}
