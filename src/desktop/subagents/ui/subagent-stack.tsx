import { RandomAvatar } from "@nessa-ui/react/random-avatar"
import { useSessionSubagents } from "../adapters/react/subagents-provider"
import { byActivity, stateCounts } from "../model/subagent"
import { count } from "../../ui/format"
import { tooltip } from "../../ui/tooltip"
import "./subagents.css"

/**
 * A conversation's subagents in a glance, for its header: the three doing
 * the most, and how many more. Nothing when it has none. It opens the
 * conversation's subagents panel.
 */
export function SubagentStack({
  sessionId,
  onOpen,
}: {
  sessionId: string
  onOpen: () => void
}) {
  const swarm = useSessionSubagents(sessionId)
  if (!swarm || swarm.subagents.length === 0) return null
  const shown = byActivity(swarm.subagents).slice(0, 3)
  const more = swarm.subagents.length - shown.length
  const working = stateCounts(swarm.subagents).working
  return (
    <button
      type="button"
      className="sa-stack"
      draggable={false}
      onClick={(event) => {
        event.stopPropagation()
        onOpen()
      }}
      {...tooltip(
        `${count(swarm.subagents.length)} subagents · ${count(working)} working`,
      )}
    >
      <span className="sa-stack-faces">
        {shown.map((subagent) => (
          <RandomAvatar
            key={subagent.id}
            seed={subagent.seed}
            ground="ink"
            className="sa-avatar"
            style={{ width: 18, height: 18 }}
          />
        ))}
      </span>
      {more > 0 ? <span className="sa-stack-more">+{count(more)}</span> : null}
    </button>
  )
}
