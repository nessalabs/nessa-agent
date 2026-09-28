import { memo } from "react"
import { AgentMark } from "../../../../onboarding/ui/agent-mark"
import { agentName, agentOf, type ModelRef } from "../../model/workspace-index"

/**
 * The agent a session runs, as its mark on a small glass tile, like an app
 * icon; an agent with no shipped mark shows a quiet monogram instead.
 */
export const AgentTile = memo(function AgentTile({
  model,
  size = 20,
}: {
  model: ModelRef
  size?: number
}) {
  const agent = agentOf(model)
  return (
    <span
      className="workspace-agent-tile"
      data-agent={agent}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.5) }}
    >
      <AgentMark id={agent} name={agentName(agent)} />
    </span>
  )
})
